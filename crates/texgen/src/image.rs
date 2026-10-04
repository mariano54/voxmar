//! RGBA images and PNG files.

/// An 8-bit sRGB image with straight alpha, rows top to bottom.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Image {
    /// A fully transparent image.
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height, pixels: vec![0; (width * height * 4) as usize] }
    }

    pub fn filled(width: u32, height: u32, colour: [u8; 4]) -> Self {
        Self { width, height, pixels: colour.repeat((width * height) as usize) }
    }

    pub fn get(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        self.pixels[i..i + 4].try_into().unwrap()
    }

    pub fn set(&mut self, x: u32, y: u32, colour: [u8; 4]) {
        let i = ((y * self.width + x) * 4) as usize;
        self.pixels[i..i + 4].copy_from_slice(&colour);
    }

    /// Alpha-blends `colour` over the pixel at `x`, `y`, if it is inside the image.
    pub fn blend(&mut self, x: i64, y: i64, colour: [u8; 4]) {
        if x < 0 || y < 0 || x >= self.width as i64 || y >= self.height as i64 || colour[3] == 0 {
            return;
        }
        let (x, y) = (x as u32, y as u32);
        if colour[3] == 255 {
            return self.set(x, y, colour);
        }
        let below = self.get(x, y);
        let a = colour[3] as f32 / 255.0;
        let b = below[3] as f32 / 255.0;
        let out = a + b * (1.0 - a);
        let mut mixed = [0u8; 4];
        for c in 0..3 {
            let value = (colour[c] as f32 * a + below[c] as f32 * b * (1.0 - a)) / out.max(1e-6);
            mixed[c] = value.round() as u8;
        }
        mixed[3] = (out * 255.0).round() as u8;
        self.set(x, y, mixed);
    }

    /// Draws `image` with its top-left corner at `x`, `y`, each pixel as a `scale`×`scale`
    /// square, blending over what is there.
    pub fn draw(&mut self, image: &Image, x: i64, y: i64, scale: u32) {
        for sy in 0..image.height * scale {
            for sx in 0..image.width * scale {
                self.blend(x + sx as i64, y + sy as i64, image.get(sx / scale, sy / scale));
            }
        }
    }

    /// Decodes a PNG of any colour type into RGBA.
    pub fn decode_png(bytes: &[u8]) -> Option<Image> {
        let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = decoder.read_info().ok()?;
        let mut buffer = vec![0; reader.output_buffer_size()?];
        let info = reader.next_frame(&mut buffer).ok()?;
        let data = &buffer[..info.buffer_size()];
        let pixels = match info.color_type {
            png::ColorType::Rgba => data.to_vec(),
            png::ColorType::Rgb => data.as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
            png::ColorType::GrayscaleAlpha => data.as_chunks::<2>().0.iter().flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
            png::ColorType::Grayscale => data.iter().flat_map(|&g| [g, g, g, 255]).collect(),
            png::ColorType::Indexed => return None,
        };
        Some(Image { width: info.width, height: info.height, pixels })
    }

    pub fn encode_png(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, self.width, self.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("PNG header");
        writer.write_image_data(&self.pixels).expect("PNG data");
        writer.finish().expect("PNG end");
        bytes
    }
}
