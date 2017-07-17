extern crate std;
use std::f64::consts;


use profile::Profile;
use bounds::Bounds;
use sun_ccfs::*;
use fit_rv::{Gaussian, fit_rv};
use simulation::normalize;

static SOLAR_RADIUS: f64 = 696000.0; // TODO: km/s
static DAYS_TO_SECONDS: f64 = 86400.0;

pub struct Star {
    pub period: f64,
    pub inclination: f64,
    pub temperature: f64,
    pub spot_temp_diff: f64,
    pub limb_linear: f64,
    pub limb_quadratic: f64,
    pub grid_size: usize,
    pub intensity: f64,
    pub flux_quiet: f64,
    pub zero_rv: f64,
    pub equatorial_velocity: f64,
    pub integrated_ccf: Vec<f64>,
    pub fit_result: Gaussian,
    pub profile_active: Profile,
    pub profile_quiet: Profile,
}

impl Star {
    pub fn new(
        radius: f64,
        period: f64,
        inclination: f64,
        temperature: f64,
        spot_temp_diff: f64,
        limb_linear: f64,
        limb_quadratic: f64,
        grid_size: usize,
    ) -> Self {

        let sqrt = f64::sqrt;

        let edge_velocity = (2.0 * consts::PI * radius * SOLAR_RADIUS) / (period * DAYS_TO_SECONDS);
        let equatorial_velocity = edge_velocity * inclination.sin();

        let profile_quiet = Profile::new(rv(), ccf_quiet());
        let profile_active = Profile::new(rv(), ccf_active());

        let mut integrated_ccf = vec![0.0; ccf_quiet().len()];
        let mut flux_quiet = 0.0;
        let grid_step = 2.0 / grid_size as f64;

        for y in (0..grid_size + 1).map(|a| (a as f64 * grid_step) - 1.0) {
            let ccf_shifted = profile_quiet.shift(y * equatorial_velocity);
            let z_bound = sqrt(1.0 - y * y);
            let limb_integral = limb_integral(
                &Bounds::new(-z_bound, z_bound),
                y,
                limb_linear,
                limb_quadratic,
            );
            for i in 0..integrated_ccf.len() {
                integrated_ccf[i] += ccf_shifted[i] * limb_integral;
            }
            flux_quiet += limb_integral;
        }

        let mut normalized = integrated_ccf.clone();
        normalize(&mut normalized);

        let guess = Gaussian {
            height: normalized[normalized.len()/2]-normalized[0],
            centroid: profile_quiet.rv[normalized.len()/2],
            width: 2.71,
            offset: normalized[0],
        };

        /*
        let guess_func = rv().iter().map(|x|
            guess.height * f64::exp(-(x - guess.centroid) * (x - guess.centroid) / (2.0 * guess.width * guess.width)) + guess.offset)
            .collect::<Vec<f64>>();

        let mut rv_fig = Figure::new();
        rv_fig.axes2d()
            .lines(&rv(), &normalized, &[Color("black")])
            .lines(&rv(), &guess_func, &[Color("red")]);
        rv_fig.show();
        */

        let initial_fit = fit_rv(&rv(), &normalized, &guess);

        Star {
            period: period,
            inclination: inclination * consts::PI / 180.0,
            temperature: temperature,
            spot_temp_diff: spot_temp_diff,
            limb_linear: limb_linear,
            limb_quadratic: limb_quadratic,
            grid_size: grid_size,
            intensity: 0.0,
            flux_quiet: flux_quiet,
            zero_rv: initial_fit.centroid,
            equatorial_velocity: equatorial_velocity,
            integrated_ccf: integrated_ccf,
            fit_result: initial_fit,
            profile_active: profile_active,
            profile_quiet: profile_quiet,
        }
    }

    pub fn limb_integral(&self, z_bounds: &Bounds, y: f64) -> f64 {
        limb_integral(z_bounds, y, self.limb_linear, self.limb_quadratic)
    }
}

pub fn min(a: f64, b: f64) -> f64 {
    if a < b {
        return a;
    }
    b
}

pub fn limb_integral(z_bounds: &Bounds, y: f64, limb_linear: f64, limb_quadratic: f64) -> f64 {
    use std::f64::EPSILON;
    if (z_bounds.lower - z_bounds.upper).abs() < EPSILON {
        return 0.0;
    }

    let z_upper = z_bounds.upper;
    let z_lower = z_bounds.lower;

    let x_upper = (1.0 - min(z_upper * z_upper + y * y, 1.0)).sqrt();
    let x_lower = (1.0 - min(z_lower * z_lower + y * y, 1.0)).sqrt();

    1. / 6. *
        (z_upper *
             (3.0 * limb_linear * (x_upper - 2.0) +
                  2.0 *
                      (limb_quadratic * (3.0 * x_upper + 3.0 * y * y + z_upper * z_upper - 6.0) +
                           3.0)) -
             3.0 * (y * y - 1.0) * (limb_linear + 2.0 * limb_quadratic) *
                 (z_upper / x_upper).atan()) -
        1. / 6. *
            (z_lower *
                 (3.0 * limb_linear * (x_lower - 2.0) +
                      2.0 *
                          (limb_quadratic *
                               (3.0 * x_lower + 3.0 * y * y + z_lower * z_lower - 6.0) +
                               3.0)) -
                 3.0 * (y * y - 1.0) * (limb_linear + 2.0 * limb_quadratic) *
                     (z_lower / x_lower).atan())
}

impl std::fmt::Debug for Star {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("")
            .field("period", &self.period)
            .field("longitude", &self.inclination)
            .field("radius", &self.temperature)
            .field("plage", &self.limb_linear)
            .field("mortal", &self.limb_quadratic)
            .field("grid_size", &self.grid_size)
            .finish()
    }
}

