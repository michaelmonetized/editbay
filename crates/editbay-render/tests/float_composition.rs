use editbay_render::{Compositor, InputTransfer, Precision, reference_with_transfer};

#[test]
fn actual_gpu_matches_independent_color_and_alpha_reference_at_both_precisions() {
    let rgba: Vec<u8> = (0..257 * 17 * 4)
        .map(|i| ((i * 73 + 19) % 256) as u8)
        .collect();
    for transfer in [InputTransfer::Srgb, InputTransfer::Bt709] {
        let expected = reference_with_transfer(&rgba, transfer);
        for (precision, tolerance) in [(Precision::Half, 0.002), (Precision::Full, 0.00002)] {
            let compositor = Compositor::with_transfer(257, 17, precision, transfer).unwrap();
            assert!(compositor.compose(&rgba[..4]).is_err());
            let actual = compositor.compose(&rgba).unwrap();
            assert_eq!(actual.len(), expected.len());
            for (index, (a, b)) in actual.iter().zip(&expected).enumerate() {
                assert!(
                    a.is_finite() && (a - b).abs() <= tolerance,
                    "channel {index}: {a} != {b}"
                );
            }
            assert_eq!(compositor.compose(&rgba).unwrap(), actual);
        }
    }
}
