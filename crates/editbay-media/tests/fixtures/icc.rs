//! Original MIT ICC/PNG fixture generator and independent analytic color oracle.
//! Binary layouts: ICC.1:2001-04 and W3C PNG iCCP; no native/CMM encoder is used.
use editbay_core::WorkingGamut;

pub type FixtureResult<T> = Result<T, Box<dyn std::error::Error>>;
type Matrix = [[f64; 3]; 3];
pub const WIDTH: u32 = 16;
pub const HEIGHT: u32 = 4;
pub const GAMMA_BITS: u16 = 461;
pub const GAMMA: f64 = GAMMA_BITS as f64 / 256.0;
const D50: [f64; 3] = [0.9642, 1.0, 0.8249];
pub const NATIVE_ALLOWANCE: f64 = 2.0e-5;
pub const ICC_SPEC: &str = "https://www.color.org/specification/ICC.1-2001-04.pdf";
pub const PNG_SPEC: &str = "https://www.w3.org/TR/png-3/#11iCCP";

/// Multiply a mathematical RGB/XYZ matrix by one vector.
/// `matrix` and `vector` supply independent scalar coefficients; returns three components.
fn apply(matrix: Matrix, vector: [f64; 3]) -> [f64; 3] {
    matrix.map(|row| row.into_iter().zip(vector).map(|(a, b)| a * b).sum())
}

/// Multiply two mathematical matrices without consulting the native color library.
/// `left` and `right` specify composition; returns their product.
fn multiply(left: Matrix, right: Matrix) -> Matrix {
    std::array::from_fn(|row| {
        std::array::from_fn(|column| (0..3).map(|k| left[row][k] * right[k][column]).sum())
    })
}

/// Invert one nonsingular fixture matrix by scalar cofactors.
/// `matrix` supplies known primaries; returns the independent inverse.
fn inverse(matrix: Matrix) -> Matrix {
    let [[a, b, c], [d, e, f], [g, h, i]] = matrix;
    let determinant = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    assert!(determinant.abs() > 1.0e-8);
    [
        [e * i - f * h, c * h - b * i, b * f - c * e],
        [f * g - d * i, a * i - c * g, c * d - a * f],
        [d * h - e * g, b * g - a * h, a * e - b * d],
    ]
    .map(|row| row.map(|v| v / determinant))
}

/// Convert chromaticity to relative XYZ at unit luminance.
/// `x` and `y` are CIE coordinates; returns the mathematical white/primary vector.
fn xy_xyz(x: f64, y: f64) -> [f64; 3] {
    [x / y, 1.0, (1.0 - x - y) / y]
}

/// Derive a D65 RGB matrix directly from the target's published chromaticities.
/// `gamut` selects primaries; returns independently scaled RGB-to-XYZ coefficients.
fn rgb_xyz(gamut: WorkingGamut) -> Matrix {
    let xy = match gamut {
        WorkingGamut::Bt709 => [[0.64, 0.33], [0.30, 0.60], [0.15, 0.06]],
        WorkingGamut::Bt2020 => [[0.708, 0.292], [0.170, 0.797], [0.131, 0.046]],
        WorkingGamut::DisplayP3 => [[0.680, 0.320], [0.265, 0.690], [0.150, 0.060]],
    };
    let columns = xy.map(|[x, y]| xy_xyz(x, y));
    let unscaled = std::array::from_fn(|row| std::array::from_fn(|column| columns[column][row]));
    let scale = apply(inverse(unscaled), xy_xyz(0.3127, 0.3290));
    std::array::from_fn(|row| std::array::from_fn(|column| unscaled[row][column] * scale[column]))
}

/// Adapt the original BT709 colorants to the ICC D50 connection space.
/// Takes no arguments; returns a Bradford-adapted matrix independently of LCMS.
fn d50_adaptation() -> Matrix {
    let cone = [
        [0.8951, 0.2664, -0.1614],
        [-0.7502, 1.7135, 0.0367],
        [0.0389, -0.0685, 1.0296],
    ];
    let source = apply(cone, xy_xyz(0.3127, 0.3290));
    let target = apply(cone, D50);
    let scaling = std::array::from_fn(|row| {
        std::array::from_fn(|column| {
            if row == column {
                target[row] / source[row]
            } else {
                0.0
            }
        })
    });
    multiply(inverse(cone), multiply(scaling, cone))
}

/// Encode an ICC signed fixed16 coefficient with round-to-nearest quantization.
/// `value` is a bounded fixture coefficient; returns four specified big-endian bytes.
fn fixed16(value: f64) -> [u8; 4] {
    assert!((-32768.0..32768.0).contains(&value));
    ((value * 65536.0).round() as i32).to_be_bytes()
}

/// Build an ICC XYZ tag directly from three analytic coordinates.
/// `xyz` supplies source/white coordinates; returns type, reserved bytes and fixed16 data.
fn xyz_tag(xyz: [f64; 3]) -> Vec<u8> {
    let mut bytes = b"XYZ \0\0\0\0".to_vec();
    bytes.extend(xyz.into_iter().flat_map(fixed16));
    bytes
}

/// Hand-encode an original v2.4 RGB matrix/TRC profile without a CMM encoder.
/// Takes no arguments; returns complete aligned profile bytes with gamma exactly 461/256.
pub fn original_profile() -> FixtureResult<Vec<u8>> {
    let matrix = multiply(d50_adaptation(), rgb_xyz(WorkingGamut::Bt709));
    let mut description = b"desc\0\0\0\0".to_vec();
    let title = b"Original EditBay gamma 461 over 256\0";
    description.extend_from_slice(&u32::try_from(title.len())?.to_be_bytes());
    description.extend_from_slice(title);
    description.extend_from_slice(&[0; 78]);
    let mut copyright = b"text\0\0\0\0".to_vec();
    copyright.extend_from_slice(b"MIT; original analytic EditBay qualification fixture\0");
    let mut curve = b"curv\0\0\0\0".to_vec();
    curve.extend_from_slice(&1_u32.to_be_bytes());
    curve.extend_from_slice(&GAMMA_BITS.to_be_bytes());
    let mut tags = vec![
        (*b"desc", description),
        (*b"cprt", copyright),
        (*b"wtpt", xyz_tag(D50)),
    ];
    for (channel, xyz_name, trc_name) in [
        (0, *b"rXYZ", *b"rTRC"),
        (1, *b"gXYZ", *b"gTRC"),
        (2, *b"bXYZ", *b"bTRC"),
    ] {
        tags.push((
            xyz_name,
            xyz_tag(std::array::from_fn(|row| matrix[row][channel])),
        ));
        tags.push((trc_name, curve.clone()));
    }
    let table_bytes = tags.len() * 12;
    let mut bytes = vec![0; 132 + table_bytes];
    bytes[8..12].copy_from_slice(&0x0240_0000_u32.to_be_bytes());
    bytes[12..16].copy_from_slice(b"mntr");
    bytes[16..20].copy_from_slice(b"RGB ");
    bytes[20..24].copy_from_slice(b"XYZ ");
    for (index, value) in [2026_u16, 10, 15, 0, 0, 0].into_iter().enumerate() {
        bytes[24 + index * 2..26 + index * 2].copy_from_slice(&value.to_be_bytes());
    }
    bytes[36..40].copy_from_slice(b"acsp");
    bytes[64..68].copy_from_slice(&1_u32.to_be_bytes());
    for (index, value) in D50.into_iter().enumerate() {
        bytes[68 + index * 4..72 + index * 4].copy_from_slice(&fixed16(value));
    }
    bytes[128..132].copy_from_slice(&u32::try_from(tags.len())?.to_be_bytes());
    for (index, (signature, data)) in tags.into_iter().enumerate() {
        while !bytes.len().is_multiple_of(4) {
            bytes.push(0);
        }
        let offset = u32::try_from(bytes.len())?;
        let table = 132 + index * 12;
        bytes[table..table + 4].copy_from_slice(&signature);
        bytes[table + 4..table + 8].copy_from_slice(&offset.to_be_bytes());
        bytes[table + 8..table + 12].copy_from_slice(&u32::try_from(data.len())?.to_be_bytes());
        bytes.extend_from_slice(&data);
    }
    while !bytes.len().is_multiple_of(4) {
        bytes.push(0);
    }
    let length = u32::try_from(bytes.len())?;
    bytes[..4].copy_from_slice(&length.to_be_bytes());
    Ok(bytes)
}

/// Generate full-range RGB16 ramps, primaries and channels with significant low bits.
/// Takes no arguments; returns original opaque pixel values in row order.
pub fn samples() -> Vec<[u16; 3]> {
    let ramp = [
        0, 1, 255, 256, 257, 4095, 8191, 16384, 32767, 32768, 49151, 65279, 65280, 65534, 65535,
        0x1235,
    ];
    (0..HEIGHT)
        .flat_map(|row| {
            (0..WIDTH as usize).map(move |column| match row {
                0 => [ramp[column]; 3],
                1 => [ramp[column], 0, 0],
                2 => [ramp[column], ramp[column * 5 % 16], ramp[column * 11 % 16]],
                _ => match column % 4 {
                    0 => [65535, 1, 0],
                    1 => [0, 65535, 257],
                    2 => [1, 0, 65535],
                    _ => [0x1235, 0x5679, 0x9abd],
                },
            })
        })
        .collect()
}

/// Compute PNG's reflected IEEE CRC independently of the media codec.
/// `bytes` covers type and payload; returns its standard CRC32.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ if crc & 1 == 0 { 0 } else { 0xedb8_8320 };
        }
    }
    !crc
}

/// Wrap tiny fixture bytes in original stored-DEFLATE/zlib with Adler32.
/// `bytes` is at most one stored block; returns the exact standard datastream.
fn zlib(bytes: &[u8]) -> FixtureResult<Vec<u8>> {
    let length = u16::try_from(bytes.len())?;
    let mut stream = vec![0x78, 0x01, 0x01];
    stream.extend_from_slice(&length.to_le_bytes());
    stream.extend_from_slice(&(!length).to_le_bytes());
    stream.extend_from_slice(bytes);
    let (mut a, mut b) = (1_u32, 0_u32);
    for byte in bytes {
        a = (a + u32::from(*byte)) % 65521;
        b = (b + a) % 65521;
    }
    stream.extend_from_slice(&((b << 16) | a).to_be_bytes());
    Ok(stream)
}

/// Append a specified genuine PNG chunk and independent checksum.
/// `output`, `kind` and `data` supply bytes; returns checked chunk-length status.
fn chunk(output: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) -> FixtureResult<()> {
    output.extend_from_slice(&u32::try_from(data.len())?.to_be_bytes());
    output.extend_from_slice(kind);
    output.extend_from_slice(data);
    let mut checked = kind.to_vec();
    checked.extend_from_slice(data);
    output.extend_from_slice(&crc32(&checked).to_be_bytes());
    Ok(())
}

/// Encode original opaque RGB16 PNG carrying the exact iCCP source profile.
/// `profile` supplies ICC bytes; returns a genuine image, without using a codec encoder.
pub fn original_png(profile: &[u8]) -> FixtureResult<Vec<u8>> {
    assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    let mut rows = Vec::new();
    for row in samples().chunks_exact(WIDTH as usize) {
        rows.push(0);
        for pixel in row {
            rows.extend(pixel.iter().flat_map(|value| value.to_be_bytes()));
        }
    }
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::new();
    header.extend_from_slice(&WIDTH.to_be_bytes());
    header.extend_from_slice(&HEIGHT.to_be_bytes());
    header.extend_from_slice(&[16, 2, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &header)?;
    let mut iccp = b"Original EditBay RGB\0\0".to_vec();
    iccp.extend_from_slice(&zlib(profile)?);
    chunk(&mut png, b"iCCP", &iccp)?;
    chunk(&mut png, b"IDAT", &zlib(&rows)?)?;
    chunk(&mut png, b"IEND", &[])?;
    Ok(png)
}

/// Derive expected linear target values and explicit profile-matrix quantization error.
/// `encoded` is straight source RGB; `gamut` selects target; returns ideal/bounded results.
pub fn expected(encoded: [f64; 3], gamut: WorkingGamut) -> ([f64; 3], [f64; 3]) {
    let source = multiply(d50_adaptation(), rgb_xyz(WorkingGamut::Bt709));
    let target_inverse = inverse(multiply(d50_adaptation(), rgb_xyz(gamut)));
    let linear = encoded.map(|value| value.powf(GAMMA));
    let ideal = apply(target_inverse, apply(source, linear));
    let quantized_source = source.map(|row| row.map(|value| (value * 65536.0).round() / 65536.0));
    let quantized = apply(target_inverse, apply(quantized_source, linear));
    let allowance = std::array::from_fn(|channel| {
        (quantized[channel] - ideal[channel]).abs() + NATIVE_ALLOWANCE
    });
    (ideal, allowance)
}
