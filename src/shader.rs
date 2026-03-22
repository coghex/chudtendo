use serde::Deserialize;
use std::path::Path;

/// A software "shader" loaded from a YAML file that describes how to
/// transform the PPU's raw framebuffer colors for display.
#[derive(Clone, Debug)]
pub struct Shader {
    pub name: String,
    /// Pre-computed lookup table: lut[out_channel][in_channel][value] -> i16
    /// contribution. Summing the three input contributions and clamping gives
    /// the final output byte.
    lut: [[[i16; 256]; 3]; 3],
}

#[derive(Deserialize)]
struct ShaderFile {
    name: String,
    color_matrix: ColorMatrix,
    #[serde(default = "default_gamma")]
    gamma: f64,
    #[serde(default = "default_brightness")]
    brightness: f64,
}

#[derive(Deserialize)]
struct ColorMatrix {
    r: [f64; 3],
    g: [f64; 3],
    b: [f64; 3],
}

fn default_gamma() -> f64 { 1.0 }
fn default_brightness() -> f64 { 1.0 }

impl Shader {
    pub fn load(path: &Path) -> Result<Self, String> {
        let contents = std::fs::read_to_string(path)
            .map_err(|e| format!("failed to read shader {}: {e}", path.display()))?;
        let file: ShaderFile = serde_yaml::from_str(&contents)
            .map_err(|e| format!("failed to parse shader {}: {e}", path.display()))?;
        Ok(Self::from_file(file))
    }

    fn from_file(file: ShaderFile) -> Self {
        let matrix = [file.color_matrix.r, file.color_matrix.g, file.color_matrix.b];
        let gamma = file.gamma;
        let brightness = file.brightness;

        let mut lut = [[[0i16; 256]; 3]; 3];

        for out_ch in 0..3 {
            for in_ch in 0..3 {
                let weight = matrix[out_ch][in_ch];
                for v in 0..256 {
                    let linear = if gamma != 1.0 {
                        (v as f64 / 255.0).powf(gamma)
                    } else {
                        v as f64 / 255.0
                    };

                    let weighted = linear * weight * brightness;

                    let encoded = if gamma != 1.0 {
                        weighted.abs().powf(1.0 / gamma).copysign(weighted)
                    } else {
                        weighted
                    };

                    lut[out_ch][in_ch][v] = (encoded * 255.0).round().clamp(-255.0, 255.0) as i16;
                }
            }
        }

        Self { name: file.name, lut }
    }

    /// Apply color correction to an RGBA framebuffer in-place.
    pub fn apply(&self, framebuffer: &mut [u8]) {
        for pixel in framebuffer.chunks_exact_mut(4) {
            let r = pixel[0] as usize;
            let g = pixel[1] as usize;
            let b = pixel[2] as usize;

            let out_r = self.lut[0][0][r] + self.lut[0][1][g] + self.lut[0][2][b];
            let out_g = self.lut[1][0][r] + self.lut[1][1][g] + self.lut[1][2][b];
            let out_b = self.lut[2][0][r] + self.lut[2][1][g] + self.lut[2][2][b];

            pixel[0] = out_r.clamp(0, 255) as u8;
            pixel[1] = out_g.clamp(0, 255) as u8;
            pixel[2] = out_b.clamp(0, 255) as u8;
        }
    }

    /// Identity shader — no color transformation.
    pub fn identity() -> Self {
        let mut lut = [[[0i16; 256]; 3]; 3];
        for ch in 0..3 {
            for v in 0..256 {
                lut[ch][ch][v] = v as i16;
            }
        }
        Self { name: "Identity".to_owned(), lut }
    }
}
