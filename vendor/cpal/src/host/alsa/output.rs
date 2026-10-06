use crate::{Error, ErrorKind};

/// Submit every frame consumed by one callback or report a visible failure.
/// `buffer`, `frames` and `stride` describe the prepared period; `write` performs
/// native writes and `suspended` inspects the device after an error. Returns only
/// after complete submission, or fails without requesting another application block.
pub(super) fn write_period(
    buffer: &[u8],
    frames: usize,
    stride: usize,
    mut write: impl FnMut(&[u8]) -> Result<usize, alsa::Error>,
    suspended: impl Fn() -> bool,
) -> Result<(), Error> {
    if frames == 0 || stride == 0 || frames.checked_mul(stride) != Some(buffer.len()) {
        return Err(ErrorKind::InvalidInput.into());
    }
    let mut written = 0;
    while written < frames {
        match write(&buffer[written * stride..]) {
            Ok(n) if n > 0 && n <= frames - written => written += n,
            Ok(_) => return Err(ErrorKind::Xrun.into()),
            Err(error) if matches!(error.errno(), libc::EAGAIN | libc::EPIPE) || suspended() => {
                return Err(ErrorKind::Xrun.into());
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_writes_submit_each_prepared_byte_once() {
        let buffer: Vec<_> = (0..20).collect();
        let mut calls = 0;
        write_period(
            &buffer,
            5,
            4,
            |remaining| {
                let offset = [0, 8, 12][calls];
                assert_eq!(remaining, &buffer[offset..]);
                let count = [2, 1, 2][calls];
                calls += 1;
                Ok(count)
            },
            || false,
        )
        .unwrap();
        assert_eq!(calls, 3);
    }

    #[test]
    fn no_progress_partial_failure_and_suspend_never_skip_consumed_frames() {
        for prefix in [0, 1] {
            for (result, suspended) in [
                (Ok(0), false),
                (Ok(6), false),
                (Err(alsa::Error::new("writei", libc::EAGAIN)), false),
                (Err(alsa::Error::new("writei", libc::EPIPE)), false),
                (Err(alsa::Error::new("writei", libc::EIO)), true),
            ] {
                let mut failure = Some(result);
                let mut calls = 0;
                let error = write_period(
                    &[0; 20],
                    5,
                    4,
                    |_| {
                        calls += 1;
                        if prefix == 1 && calls == 1 {
                            Ok(1)
                        } else {
                            failure.take().unwrap()
                        }
                    },
                    || suspended,
                )
                .unwrap_err();
                assert_eq!(error.kind(), ErrorKind::Xrun);
                assert_eq!(calls, prefix + 1);
            }
        }
    }

    #[test]
    fn malformed_periods_and_other_device_errors_fail_before_more_consumption() {
        for (frames, stride) in [(0, 4), (5, 0), (6, 4), (usize::MAX, 4)] {
            assert_eq!(
                write_period(
                    &[0; 20],
                    frames,
                    stride,
                    |_| panic!("invalid period written"),
                    || false
                )
                .unwrap_err()
                .kind(),
                ErrorKind::InvalidInput
            );
        }
        let error = write_period(
            &[0; 20],
            5,
            4,
            |_| Err(alsa::Error::new("writei", libc::ENODEV)),
            || false,
        )
        .unwrap_err();
        assert_ne!(error.kind(), ErrorKind::Xrun);
    }
}
