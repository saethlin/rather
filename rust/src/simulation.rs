use std::iter;
use std::sync::RwLock;
use std::fmt::Write;
use ini::Ini;
use rand::distributions::{IndependentSample, LogNormal, Range};
use std::sync::Arc;
use planck::planck_integral;
use fit_rv::fit_rv;
use compute_bisector::compute_bisector;
use rayon::prelude::*;

use star::Star;
use spot::Spot;

/// An observed radial velocity and line bisector.
pub struct Observation {
    /// The radial velocity value in m/s.
    pub rv: f64,
    /// The line bisector in m/s.
    pub bisector: Vec<f64>,
}

/// A model of a star with spots that can be observed.
#[derive(Derivative)]
#[derivative(Debug)]
pub struct Simulation {
    #[doc(hidden)]
    pub star: Arc<Star>,
    #[doc(hidden)]
    pub spots: Vec<Spot>,
    dynamic_fill_factor: f64,
    #[derivative(Debug = "ignore")]
    generator: Arc<RwLock<::rand::StdRng>>,
}

macro_rules! get {
(&mut $error:ident, $file:ident, $filename:ident, $section:expr, $field:expr, $type:ty) => (
    { let default_string = "1.0".to_owned();
    $file.section(Some($section)).unwrap().get($field)
        .unwrap_or_else(|| {
            writeln!(
                $error,
                "Missing field {} of section {} in config file {}",
                $field,
                $section,
                $filename).unwrap();
            &default_string
        })
        .parse::<$type>()
        .unwrap_or_else(|_| {
            writeln!(
                $error,
                "Cannot parse field {} of section {} in config file {}",
                $field,
                $section,
                $filename).unwrap();
            <$type as Default>::default()
        })
    }
)
}

impl Simulation {
    /// Construct a new Star from a config file.
    pub fn new(filename: &str) -> Simulation {
        let mut error = String::new();
        let file = Ini::load_from_file(filename)
            .expect(&format!("Could not open config file {}", filename));

        file.section(Some("star")).expect(&format!(
            "Missing section start in config file {}",
            filename
        ));

        let radius = get!(&mut error, file, filename, "star", "radius", f64);
        let period = get!(&mut error, file, filename, "star", "period", f64);
        let inclination = get!(&mut error, file, filename, "star", "inclination", f64);
        let temperature = get!(&mut error, file, filename, "star", "Tstar", f64);
        let spot_temp_diff = get!(&mut error, file, filename, "star", "Tdiff_spot", f64);
        let limb_linear = get!(&mut error, file, filename, "star", "limb1", f64);
        let limb_quadratic = get!(&mut error, file, filename, "star", "limb2", f64);
        let dynamic_fill_factor = get!(&mut error, file, filename, "star", "fillfactor", f64);
        let grid_size = get!(&mut error, file, filename, "star", "grid_resolution", usize);

        let star = Arc::new(Star::new(
            radius,
            period,
            inclination,
            temperature,
            spot_temp_diff,
            limb_linear,
            limb_quadratic,
            grid_size,
        ));

        let spots: Vec<Spot> = file.iter()
            .filter(|&(s, _)| s.to_owned().is_some())
            .filter(|&(s, _)| s.to_owned().unwrap().as_str().starts_with("spot"))
            .map(|(section, _)| {
                let sec = section.clone().unwrap();
                let latitude = get!(&mut error, file, filename, sec.as_str(), "latitude", f64);
                let longitude = get!(&mut error, file, filename, sec.as_str(), "longitude", f64);
                let size = get!(&mut error, file, filename, sec.as_str(), "size", f64);
                let plage = get!(&mut error, file, filename, sec.as_str(), "plage", bool);

                Spot::new(star.clone(), latitude, longitude, size, plage, false)
            })
            .collect();

        if !error.is_empty() {
            panic!("One or more errors loading config file");
        }

        Simulation {
            star: star,
            spots: spots,
            dynamic_fill_factor: dynamic_fill_factor,
            generator: Arc::new(RwLock::new(::rand::StdRng::new().unwrap())),
        }
    }

    fn check_fill_factor(&mut self, time: f64) {
        let mut current_fill_factor = self.spots
            .iter()
            .filter(|s| s.alive(time))
            .map(|s| (s.radius * s.radius) / 2.0)
            .sum::<f64>();

        let fill_range = LogNormal::new(0.5, 4.0);
        let lat_range = Range::new(-30.0, 30.0);
        let long_range = Range::new(0.0, 360.0);

        if current_fill_factor < self.dynamic_fill_factor {
            let mut generator = self.generator
                .write()
                .expect("Simulation RNG lock was poisoned by another panic");

            while current_fill_factor < self.dynamic_fill_factor {
                let new_fill_factor = iter::repeat(())
                    .map(|_| fill_range.ind_sample(&mut *generator) * 9.4e-6)
                    .find(|v| *v < 0.001)
                    .unwrap();

                let mut new_spot = Spot::new(
                    self.star.clone(),
                    lat_range.ind_sample(&mut *generator),
                    long_range.ind_sample(&mut *generator),
                    new_fill_factor,
                    false,
                    true,
                );
                new_spot.time_appear += time;
                new_spot.time_disappear += time;

                let collides = self.spots
                    .iter()
                    .filter(|s| s.alive(new_spot.time_appear) || s.alive(new_spot.time_disappear))
                    .any(|s| new_spot.collides_with(s));

                if !collides {
                    current_fill_factor += (new_spot.radius * new_spot.radius) / 2.0;
                    self.spots.push(new_spot);
                }
            }
        }
    }

    /// Computes the relative brightness of this system at each time (in days),
    /// when observed in the wavelength band between `wavelength_min` and `wavelength_max`.
    pub fn observe_flux(
        &mut self,
        time: &[f64],
        wavelength_min: f64,
        wavelength_max: f64,
    ) -> Vec<f64> {
        let star_intensity = planck_integral(self.star.temperature, wavelength_min, wavelength_max);
        for spot in &mut self.spots {
            spot.intensity =
                planck_integral(spot.temperature, wavelength_min, wavelength_max) / star_intensity;
        }
        for t in time.iter() {
            self.check_fill_factor(*t);
        }

        time.par_iter()
            .map(|t| {
                let spot_flux: f64 = self.spots.iter().map(|s| s.get_flux(*t)).sum();
                (self.star.flux_quiet - spot_flux) / self.star.flux_quiet
            })
            .collect()
    }

    /// Computes the radial velocity and line bisector of this system at each time (in days),
    /// when observed in the wavelength band between `wavelength_min` and `wavelength_max`.
    pub fn observe_rv(
        &mut self,
        time: &[f64],
        wavelength_min: f64,
        wavelength_max: f64,
    ) -> Vec<Observation> {
        let star_intensity = planck_integral(self.star.temperature, wavelength_min, wavelength_max);
        for spot in &mut self.spots {
            spot.intensity =
                planck_integral(spot.temperature, wavelength_min, wavelength_max) / star_intensity;
        }

        for t in time.iter() {
            self.check_fill_factor(*t);
        }

        time.par_iter()
            .map(|t| {
                let mut spot_profile = vec![0.0; self.star.profile_active.len()];
                for spot in self.spots.iter().filter(|s| s.alive(*t)) {
                    let profile = spot.get_ccf(*t);
                    for (total, this) in spot_profile.iter_mut().zip(profile.iter()) {
                        *total += *this;
                    }
                }

                for (spot, star) in spot_profile.iter_mut().zip(self.star.integrated_ccf.iter()) {
                    *spot = *star - *spot;
                }

                /*
                use resolution::set_resolution;
                let spot_profile = set_resolution(&self.star.profile_active.rv, &spot_profile);
                println!("{:?}", spot_profile);
                panic!();
                */

                let rv = fit_rv(&self.star.profile_quiet.rv, &spot_profile) - self.star.zero_rv;

                let bisector: Vec<f64> = compute_bisector(&self.star.profile_quiet.rv, &spot_profile)
                        .iter()
                        // TODO: Should I actually return the points that come back from this?
                        // Do the Y values actually matter?
                        //.map(|b| b.x - self.star.zero_rv)
                        .map(|b| b - self.star.zero_rv)
                        .collect();

                Observation {
                    rv: rv,
                    bisector: bisector,
                }
            })
            .collect()
    }

    /// Draw the simulation in a row-major fashion, as it would be seen in the visible
    /// wavelength band, 4000-7000 Angstroms.
    pub fn draw_rgba(&mut self, time: f64, image: &mut Vec<u8>) {
        // This is slow because the image is row-major, but we navigate the simulation in
        // a column-major fashion to follow the rotational symmetry
        use boundingshape::BoundingShape;
        use linspace::floatrange;
        self.check_fill_factor(time);
        let star_intensity = planck_integral(self.star.temperature, 4000e-10, 7000e-10);
        for spot in &mut self.spots {
            spot.intensity = planck_integral(spot.temperature, 4000e-10, 7000e-10) / star_intensity;
        }

        let grid_interval = 2.0 / self.star.grid_size as f64;

        for spot in self.spots.iter().filter(|s| s.alive(time)) {
            let bounds = BoundingShape::new(spot, time);
            let mut current_z_bounds = None;
            if let Some(y_bounds) = bounds.y_bounds() {
                for y in floatrange(
                    (y_bounds.lower / grid_interval).round() * grid_interval,
                    (y_bounds.upper / grid_interval).round() * grid_interval,
                    grid_interval,
                ) {
                    let y_index = ((y + 1.0) / 2.0 * 1000.0).round() as usize;
                    if let Some(z_bounds) = bounds.z_bounds(y, &mut current_z_bounds) {
                        for z in floatrange(
                            (z_bounds.lower / grid_interval).round() * grid_interval,
                            (z_bounds.upper / grid_interval).round() * grid_interval,
                            grid_interval,
                        ) {
                            let x = 1.0 - (y * y + z * z);
                            let x = f64::max(0.0, x);
                            let intensity = self.star.limb_brightness(x) * spot.intensity;
                            let z_index = ((-z + 1.0) / 2.0 * 1000.0).round() as usize;
                            let index = (z_index * 1000 + y_index) as usize;
                            image[4 * index] = (intensity * 255.0) as u8;
                            image[4 * index + 1] = (intensity * 131.0) as u8;
                            image[4 * index + 2] = 0;
                            image[4 * index + 3] = 255;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_config() {
        let sim = Simulation::new("sun.cfg");
        assert_eq!(sim.dynamic_fill_factor, 0.0);
    }
}