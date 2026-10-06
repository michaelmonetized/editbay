use editbay_core::SoundProfile;
use serde::Serialize;

/// An explicit listening route; source and delivery channels remain unchanged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub enum MonitorRoute {
    Original,
    #[default]
    Stereo,
}

pub(crate) struct MonitorMatrix {
    input: usize,
    output: usize,
    weights: Vec<f64>,
}

impl MonitorMatrix {
    /// Prepare one declared monitor route outside the device callback.
    /// `source`, `output` and `route` select original identities and device width.
    /// Returns a bounded matrix. Stereo monitoring splits center and surrounds
    /// at -3 dB and omits LFE; a lone center channel feeds both outputs at unity.
    pub(crate) fn new(
        source: &SoundProfile,
        output: u16,
        route: MonitorRoute,
    ) -> Result<Self, String> {
        let input = source.channels.len();
        let output = usize::from(output);
        if input == 0 || input > 64 || !(1..=2).contains(&output) {
            return Err("Monitoring needs a mono or stereo output device".into());
        }
        let mut weights = vec![0.; input * output];
        match route {
            MonitorRoute::Original => {
                if !((output == 1 && source.channels == ["FC"])
                    || (output == 2 && source.channels == ["FL", "FR"]))
                {
                    return Err("Original channels do not match the device; choose Stereo monitor for listening".into());
                }
                for channel in 0..output {
                    weights[channel * input + channel] = 1.;
                }
            }
            MonitorRoute::Stereo => {
                if output != 2 {
                    return Err("Stereo monitoring needs a two-channel output".into());
                }
                for (index, channel) in source.channels.iter().enumerate() {
                    let (left, right) = match channel.as_str() {
                        "FL" => (1., 0.),
                        "FR" => (0., 1.),
                        "FC" if input == 1 => (1., 1.),
                        "FC" => (
                            std::f64::consts::FRAC_1_SQRT_2,
                            std::f64::consts::FRAC_1_SQRT_2,
                        ),
                        "BL" | "SL" => (std::f64::consts::FRAC_1_SQRT_2, 0.),
                        "BR" | "SR" => (0., std::f64::consts::FRAC_1_SQRT_2),
                        "LFE" => (0., 0.),
                        _ => {
                            return Err(format!(
                                "Stereo monitor has no declared route for {channel}"
                            ));
                        }
                    };
                    weights[index] = left;
                    weights[input + index] = right;
                }
            }
        }
        Ok(Self {
            input,
            output,
            weights,
        })
    }

    /// Fill a preallocated listening block without changing rendered source PCM.
    /// `input` uses original channels and `output` has matching frame count.
    /// Returns the number of clipped device samples, or rejects malformed sound.
    pub(crate) fn apply(&self, input: &[f32], output: &mut [f32]) -> Result<u64, &'static str> {
        if !input.len().is_multiple_of(self.input)
            || output.len() != input.len() / self.input * self.output
        {
            return Err("Monitor block has the wrong channel layout");
        }
        let mut clipped = 0;
        for (source, destination) in input
            .chunks_exact(self.input)
            .zip(output.chunks_exact_mut(self.output))
        {
            for (channel, sample) in destination.iter_mut().enumerate() {
                let sum: f64 = source
                    .iter()
                    .zip(&self.weights[channel * self.input..(channel + 1) * self.input])
                    .map(|(value, weight)| f64::from(*value) * weight)
                    .sum();
                if !sum.is_finite() {
                    return Err("Monitor sound is not finite");
                }
                clipped += u64::from(sum.abs() > 1.);
                *sample = sum.clamp(-1., 1.) as f32;
            }
        }
        Ok(clipped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(channels: &[&str]) -> SoundProfile {
        SoundProfile {
            sample_rate: 48000,
            channels: channels.iter().map(|c| (*c).into()).collect(),
        }
    }

    #[test]
    fn mono_stereo_and_explicit_surround_monitor_routes_preserve_channel_positions() {
        let mono = MonitorMatrix::new(&profile(&["FC"]), 2, MonitorRoute::Stereo).unwrap();
        let mut stereo = [0.; 4];
        assert_eq!(mono.apply(&[0.25, -0.75], &mut stereo).unwrap(), 0);
        assert_eq!(stereo, [0.25, 0.25, -0.75, -0.75]);
        let surround = MonitorMatrix::new(
            &profile(&["FL", "FR", "FC", "LFE", "BL", "BR"]),
            2,
            MonitorRoute::Stereo,
        )
        .unwrap();
        for index in 0..6 {
            let mut input = [0.; 6];
            input[index] = 0.5;
            let mut output = [0.; 2];
            surround.apply(&input, &mut output).unwrap();
            let half = 0.5 * std::f32::consts::FRAC_1_SQRT_2;
            assert_eq!(
                output,
                [
                    [0.5, 0.],
                    [0., 0.5],
                    [half, half],
                    [0., 0.],
                    [half, 0.],
                    [0., half]
                ][index]
            );
        }
    }

    #[test]
    fn monitor_reports_clipping_and_rejects_undeclared_routes_or_malformed_blocks() {
        let matrix =
            MonitorMatrix::new(&profile(&["FL", "FR"]), 2, MonitorRoute::Original).unwrap();
        let mut out = [0.; 2];
        assert_eq!(matrix.apply(&[2., -2.], &mut out).unwrap(), 2);
        assert_eq!(out, [1., -1.]);
        assert!(matrix.apply(&[f32::NAN, 0.], &mut out).is_err());
        assert!(matrix.apply(&[1.], &mut out).is_err());
        assert!(MonitorMatrix::new(&profile(&["FC"]), 2, MonitorRoute::Original).is_err());
        assert!(MonitorMatrix::new(&profile(&["TFL", "TFR"]), 2, MonitorRoute::Stereo).is_err());
        assert!(MonitorMatrix::new(&profile(&["FC"]), 1, MonitorRoute::Stereo).is_err());
    }
}
