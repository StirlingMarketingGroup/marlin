use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use base64::Engine as _;
use image::{Rgba, RgbaImage};

use super::super::ThumbnailRequest;

#[derive(Clone, Copy, Debug, Default)]
struct Vec3 {
    x: f32,
    y: f32,
    z: f32,
}

impl Vec3 {
    fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
    fn sub(self, o: Self) -> Self {
        Self::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
    fn mul(self, s: f32) -> Self {
        Self::new(self.x * s, self.y * s, self.z * s)
    }
    fn dot(self, o: Self) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    fn cross(self, o: Self) -> Self {
        Self::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }
    fn len(self) -> f32 {
        self.dot(self).sqrt()
    }
    fn normalize(self) -> Self {
        let l = self.len();
        if l > 0.0 {
            self.mul(1.0 / l)
        } else {
            self
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Tri {
    v0: Vec3,
    v1: Vec3,
    v2: Vec3,
}

pub struct StlGenerator;

impl StlGenerator {
    pub fn generate(request: &ThumbnailRequest) -> Result<(String, bool), String> {
        let path = Path::new(&request.path);
        if !path.exists() {
            return Err("STL file does not exist".into());
        }

        // Parse STL into triangles
        let tris = parse_stl(path)?;
        if tris.is_empty() {
            return Err("No triangles in STL".into());
        }

        // Normalize to unit cube centered at origin
        let mut min = Vec3::new(f32::INFINITY, f32::INFINITY, f32::INFINITY);
        let mut max = Vec3::new(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
        for t in &tris {
            for v in [t.v0, t.v1, t.v2] {
                min.x = min.x.min(v.x);
                min.y = min.y.min(v.y);
                min.z = min.z.min(v.z);
                max.x = max.x.max(v.x);
                max.y = max.y.max(v.y);
                max.z = max.z.max(v.z);
            }
        }
        let center = Vec3::new(
            (min.x + max.x) * 0.5,
            (min.y + max.y) * 0.5,
            (min.z + max.z) * 0.5,
        );
        let extent = Vec3::new(max.x - min.x, max.y - min.y, max.z - min.z);
        let scale = 1.0 / extent.x.max(extent.y).max(extent.z).max(1e-6);

        // Pre-rotate for an isometric-style view
        let deg = |d: f32| d * std::f32::consts::PI / 180.0;
        let yaw = deg(35.0); // around Y
        let pitch = deg(25.0); // around X
        let rot_y = |v: Vec3| -> Vec3 {
            let (sy, cy) = yaw.sin_cos();
            Vec3::new(v.x * cy + v.z * sy, v.y, -v.x * sy + v.z * cy)
        };
        let rot_x = |v: Vec3| -> Vec3 {
            let (sx, cx) = pitch.sin_cos();
            Vec3::new(v.x, v.y * cx - v.z * sx, v.y * sx + v.z * cx)
        };

        // Two-pass: compute bounds in screen space after rotation
        let mut min2 = Vec3::new(f32::INFINITY, f32::INFINITY, f32::INFINITY);
        let mut max2 = Vec3::new(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
        for t in &tris {
            for v in [t.v0, t.v1, t.v2] {
                let p = rot_x(rot_y(v.sub(center).mul(scale)));
                min2.x = min2.x.min(p.x);
                min2.y = min2.y.min(p.y);
                min2.z = min2.z.min(p.z);
                max2.x = max2.x.max(p.x);
                max2.y = max2.y.max(p.y);
                max2.z = max2.z.max(p.z);
            }
        }

        let target = request.size.max(32);
        let pad = ((target as f32) * 0.10).round();
        let inner = (target as f32 - 2.0 * pad).max(1.0);
        let w2 = (max2.x - min2.x).max(1e-6);
        let h2 = (max2.y - min2.y).max(1e-6);
        let s2 = (inner / w2).min(inner / h2);
        let offx = -(min2.x + max2.x) * 0.5;
        let offy = -(min2.y + max2.y) * 0.5;

        // Prepare buffers
        let mut img: RgbaImage = RgbaImage::from_pixel(target, target, Rgba([0, 0, 0, 0]));
        let mut zbuf = vec![f32::INFINITY; (target as usize) * (target as usize)];

        // Simple light and color
        let light_dir = Vec3::new(-0.45, 0.80, 0.35).normalize();
        let base = [66u8, 175u8, 160u8]; // teal-ish

        // Rasterize triangles
        for t in &tris {
            // Transform vertices
            let v = [t.v0, t.v1, t.v2];
            let mut p = [Vec3::default(); 3];
            for i in 0..3 {
                let q = rot_x(rot_y(v[i].sub(center).mul(scale)));
                p[i] = q;
            }

            // Face normal in view space
            let fnrm = (p[1].sub(p[0])).cross(p[2].sub(p[0])).normalize();
            // Backface culling (camera looks along +Z towards origin in view space)
            if fnrm.z >= 0.0 {
                continue;
            }

            // Project to screen (orthographic onto XY)
            let mut sx = [0f32; 3];
            let mut sy = [0f32; 3];
            let mut sz = [0f32; 3];
            for i in 0..3 {
                let x = (p[i].x + offx) * s2;
                let y = (p[i].y + offy) * s2;
                sx[i] = (x + 0.0) + (target as f32) * 0.5;
                sy[i] = (-(y) + 0.0) + (target as f32) * 0.5; // invert Y for image space
                sz[i] = p[i].z; // depth
            }

            // Triangle bounding box
            let minx = sx
                .iter()
                .cloned()
                .fold(f32::INFINITY, f32::min)
                .floor()
                .max(0.0) as i32;
            let maxx = sx
                .iter()
                .cloned()
                .fold(f32::NEG_INFINITY, f32::max)
                .ceil()
                .min((target - 1) as f32) as i32;
            let miny = sy
                .iter()
                .cloned()
                .fold(f32::INFINITY, f32::min)
                .floor()
                .max(0.0) as i32;
            let maxy = sy
                .iter()
                .cloned()
                .fold(f32::NEG_INFINITY, f32::max)
                .ceil()
                .min((target - 1) as f32) as i32;
            if minx > maxx || miny > maxy {
                continue;
            }

            // Precompute edge function coefficients
            let e = |x0: f32, y0: f32, x1: f32, y1: f32, x: f32, y: f32| -> f32 {
                (x - x0) * (y1 - y0) - (y - y0) * (x1 - x0)
            };
            let area = e(sx[0], sy[0], sx[1], sy[1], sx[2], sy[2]);
            if area.abs() < 1e-4 {
                continue;
            }

            // Lambert shading
            let ndotl = (fnrm.mul(-1.0)).dot(light_dir).max(0.0);
            let shade = 0.18 + 0.82 * ndotl;
            let col = [
                (base[0] as f32 * shade).min(255.0) as u8,
                (base[1] as f32 * shade).min(255.0) as u8,
                (base[2] as f32 * shade).min(255.0) as u8,
                255u8,
            ];

            for y in miny..=maxy {
                for x in minx..=maxx {
                    let xf = x as f32 + 0.5;
                    let yf = y as f32 + 0.5;
                    // Barycentric weights
                    let w0 = e(sx[1], sy[1], sx[2], sy[2], xf, yf);
                    let w1 = e(sx[2], sy[2], sx[0], sy[0], xf, yf);
                    let w2 = e(sx[0], sy[0], sx[1], sy[1], xf, yf);
                    // Accept if all have same sign as area (inside)
                    if (w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0 && area > 0.0)
                        || (w0 <= 0.0 && w1 <= 0.0 && w2 <= 0.0 && area < 0.0)
                    {
                        // Normalize and interpolate depth
                        let inv_area = 1.0 / area;
                        let b0 = w0 * inv_area;
                        let b1 = w1 * inv_area;
                        let b2 = w2 * inv_area;
                        let z = b0 * sz[0] + b1 * sz[1] + b2 * sz[2];
                        let idx = (y as usize) * (target as usize) + (x as usize);
                        if z < zbuf[idx] {
                            zbuf[idx] = z;
                            img.put_pixel(x as u32, y as u32, Rgba(col));
                        }
                    }
                }
            }
        }

        // Encode as PNG data URL via image crate
        let mut out = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut out);
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut cursor, image::ImageOutputFormat::Png)
            .map_err(|e| format!("Failed to encode PNG: {}", e))?;

        let data_url = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(out)
        );
        // STL renders typically have transparent backgrounds
        Ok((data_url, true))
    }
}

fn parse_stl(path: &Path) -> Result<Vec<Tri>, String> {
    // Read all bytes
    let mut f = File::open(path).map_err(|e| format!("Failed to open STL: {}", e))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)
        .map_err(|e| format!("Failed to read STL: {}", e))?;

    if buf.len() >= 84 {
        // Check binary layout
        let tri_count = u32::from_le_bytes([buf[80], buf[81], buf[82], buf[83]]) as usize;
        let expected = 84usize + 50usize.saturating_mul(tri_count);
        if expected == buf.len() {
            return parse_binary_stl(&buf[84..], tri_count);
        }
    }
    // Fallback to ASCII
    parse_ascii_stl(&buf)
}

fn parse_binary_stl(data: &[u8], tri_count: usize) -> Result<Vec<Tri>, String> {
    let mut tris = Vec::with_capacity(tri_count.min(200_000));
    let mut i = 0usize;
    for _ in 0..tri_count {
        if i + 50 > data.len() {
            break;
        }
        let n = read_vec3_le(&data[i..]);
        i += 12;
        let v0 = read_vec3_le(&data[i..]);
        i += 12;
        let v1 = read_vec3_le(&data[i..]);
        i += 12;
        let v2 = read_vec3_le(&data[i..]);
        i += 12;
        // skip attribute byte count
        i += 2;
        // Some files have invalid normals; recompute if needed
        // keep normal computation for backface culling shading, but we won't store it
        let _nrm = {
            let mut nn = n.normalize();
            if nn.len() == 0.0 {
                nn = (v1.sub(v0)).cross(v2.sub(v0)).normalize();
            }
            nn
        };
        tris.push(Tri { v0, v1, v2 });
        if tris.len() >= 500_000 {
            break;
        } // hard safety cap
    }
    Ok(tris)
}

fn read_vec3_le(data: &[u8]) -> Vec3 {
    let x = f32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    let y = f32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    let z = f32::from_le_bytes([data[8], data[9], data[10], data[11]]);
    Vec3::new(x, y, z)
}

fn parse_ascii_stl(bytes: &[u8]) -> Result<Vec<Tri>, String> {
    let s = std::str::from_utf8(bytes).map_err(|e| format!("Invalid UTF-8 in ASCII STL: {}", e))?;
    let reader = BufReader::new(s.as_bytes());
    let mut tris: Vec<Tri> = Vec::new();
    let mut verts: Vec<Vec3> = Vec::new();
    let mut cur_normal = Vec3::new(0.0, 0.0, 1.0);

    for line in reader.lines() {
        let line = line.map_err(|e| format!("Read error: {}", e))?;
        let t = line.trim();
        if t.starts_with("facet normal") {
            let parts: Vec<&str> = t.split_whitespace().collect();
            if parts.len() >= 5 {
                let nx = parts[2].parse::<f32>().unwrap_or(0.0);
                let ny = parts[3].parse::<f32>().unwrap_or(0.0);
                let nz = parts[4].parse::<f32>().unwrap_or(1.0);
                cur_normal = Vec3::new(nx, ny, nz).normalize();
            }
            verts.clear();
        } else if t.starts_with("vertex") {
            let parts: Vec<&str> = t.split_whitespace().collect();
            if parts.len() >= 4 {
                let x = parts[1].parse::<f32>().unwrap_or(0.0);
                let y = parts[2].parse::<f32>().unwrap_or(0.0);
                let z = parts[3].parse::<f32>().unwrap_or(0.0);
                verts.push(Vec3::new(x, y, z));
            }
        } else if t.starts_with("endfacet") {
            if verts.len() >= 3 {
                let v0 = verts[0];
                let v1 = verts[1];
                let v2 = verts[2];
                let mut n = cur_normal;
                if n.len() == 0.0 {
                    n = (v1.sub(v0)).cross(v2.sub(v0)).normalize();
                }
                let _ = n; // computed for consistency; not stored
                tris.push(Tri { v0, v1, v2 });
            }
            verts.clear();
        }
        if tris.len() >= 500_000 {
            break;
        }
    }

    Ok(tris)
}
