// SPDX-License-Identifier: GPL-3.0-or-later
//! Synthetic oracle: expected pixels come from sensor samples before encoding.
//! The bit-list recurrence deliberately does not use the driver decoder.
use anyhow::{Result, ensure};
use std::{collections::BTreeMap, fs, path::Path};
const WIDTH: usize = 384;
const HEIGHT: usize = 290;

pub fn run(root: &Path, check: bool) -> Result<()> {
    for (name, bytes) in generate() {
        let path = root.join("fixtures").join(&name);
        if check {
            ensure!(
                fs::read(&path).ok().as_deref() == Some(bytes.as_slice()),
                "fixture differs: {name}"
            );
        } else {
            fs::create_dir_all(path.parent().expect("fixture parent"))?;
            fs::write(path, bytes)?;
        }
    }
    println!(
        "synthetic fixtures {}",
        if check { "verified" } else { "generated" }
    );
    Ok(())
}
fn register(value: u32) -> [u8; 32] {
    std::array::from_fn(|i| ((value >> i) & 1) as u8)
}
fn tick(bits: &mut [u8; 32]) {
    let parity = (0..32)
        .filter(|i| (0x9248_144du32 >> i) & 1 != 0)
        .map(|i| bits[i])
        .sum::<u8>()
        % 2;
    bits.rotate_left(1);
    bits[31] = parity;
}
fn pack(
    name: &str,
    descriptors: &[(u8, usize)],
    initial: u8,
    scrambled: bool,
) -> BTreeMap<String, Vec<u8>> {
    let source_rows: usize = descriptors
        .iter()
        .filter(|(flags, _)| flags & 1 == 0)
        .map(|(_, rows)| rows)
        .sum();
    let mut random = 0x57b1_9a03u32;
    let mut sensor: Vec<u8> = (0..source_rows * WIDTH)
        .map(|i| {
            if scrambled {
                random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (random >> 24) as u8
            } else {
                (30 + (i / WIDTH) * 17) as u8
            }
        })
        .collect();
    let mut cursor = 0;
    for &(flags, rows) in descriptors {
        if scrambled && flags & 6 == 2 {
            sensor[(cursor + rows) * WIDTH - 1] = 0;
        }
        if flags & 1 == 0 {
            cursor += rows;
        }
    }
    let key = |index| match index {
        254 => 0x1367_9245,
        255 => 0x89ab_cdef,
        0 => 0xf00d_cafe,
        _ => unreachable!("synthetic key index"),
    };
    let mut wire = sensor.clone();
    let mut bits = if scrambled {
        register(key(initial))
    } else {
        [0; 32]
    };
    let mut index = initial;
    cursor = 0;
    for &(flags, rows) in descriptors {
        if scrambled {
            if flags & 0x80 != 0 {
                index = index.wrapping_add(1);
                bits = register(key(index));
            }
            let length = rows * WIDTH;
            let start = cursor * WIDTH;
            match flags & 6 {
                2 => {
                    wire[start] = 0xa7;
                    for sample in 0..length - 1 {
                        let mask = [4, 8, 11, 14, 18, 21, 24, 29]
                            .iter()
                            .enumerate()
                            .map(|(i, &tap)| bits[tap] << i)
                            .sum::<u8>();
                        wire[start + sample + 1] = sensor[start + sample] ^ mask;
                        tick(&mut bits);
                    }
                    tick(&mut bits);
                }
                0 => {
                    for _ in 0..length {
                        tick(&mut bits);
                    }
                }
                _ => {}
            }
        }
        if flags & 1 == 0 {
            cursor += rows;
        }
    }
    let mut canvas = vec![0u8; WIDTH * HEIGHT];
    let (mut source, mut destination) = (0, 0);
    for &(flags, rows) in descriptors {
        let length = rows * WIDTH;
        canvas[destination..destination + length].copy_from_slice(&sensor[source..source + length]);
        destination += length;
        if flags & 1 == 0 {
            source += length;
        }
    }
    let expected = canvas.into_iter().rev().map(|value| 255 - value).collect();
    let mut header = vec![0; 64];
    header[4..6].copy_from_slice(&(source_rows as u16).to_le_bytes());
    header[6] = initial;
    for (i, &(flags, rows)) in descriptors.iter().enumerate() {
        header[16 + 2 * i] = flags;
        header[17 + 2 * i] = rows as u8;
    }
    let sums: Vec<i64> = (0..WIDTH)
        .map(|i| i64::from(wire[i]) + i64::from(wire[WIDTH + i]))
        .collect();
    let mean = sums.iter().sum::<i64>() / WIDTH as i64;
    let variance = sums.iter().map(|v| (v - mean).pow(2)).sum::<i64>() / WIDTH as i64;
    assert_eq!(variance >= 5000, scrambled);
    header.extend(wire);
    BTreeMap::from([
        (format!("{name}.wire"), header),
        (format!("{name}.gray"), expected),
    ])
}
fn generate() -> BTreeMap<String, Vec<u8>> {
    let mut assets = pack("synthetic-plain", &[(0, 3)], 0, false);
    assets.extend(pack(
        "synthetic-scrambled",
        &[(2, 2), (1, 1), (0, 1), (0x82, 2), (0x86, 1), (2, 1), (4, 1)],
        254,
        true,
    ));
    assets
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rust_generator_reproduces_all_four_independent_vectors() {
        let actual = generate();
        assert_eq!(actual.len(), 4);
        for (name, expected) in [
            (
                "synthetic-plain.wire",
                include_bytes!("../../fixtures/synthetic-plain.wire").as_slice(),
            ),
            (
                "synthetic-plain.gray",
                include_bytes!("../../fixtures/synthetic-plain.gray").as_slice(),
            ),
            (
                "synthetic-scrambled.wire",
                include_bytes!("../../fixtures/synthetic-scrambled.wire").as_slice(),
            ),
            (
                "synthetic-scrambled.gray",
                include_bytes!("../../fixtures/synthetic-scrambled.gray").as_slice(),
            ),
        ] {
            assert_eq!(actual[name], expected);
        }
    }
    #[test]
    fn check_reports_missing_or_changed_fixtures_without_rewriting() {
        let dir = crate::common::TempDir::new("xtask-fixtures").unwrap();
        assert!(run(dir.path(), true).is_err());
        run(dir.path(), false).unwrap();
        run(dir.path(), true).unwrap();
        let path = dir.path().join("fixtures/synthetic-plain.wire");
        std::fs::write(&path, b"changed").unwrap();
        assert!(run(dir.path(), true).is_err());
        assert_eq!(std::fs::read(path).unwrap(), b"changed");
    }
}
