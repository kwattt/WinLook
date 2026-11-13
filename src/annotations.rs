use crate::capture::ScreenRect;
use image::RgbaImage;

/// Available drawing tools inside a WinLook selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawTool {
    Pen,
    Arrow,
    Line,
    Rect,
    Ellipse,
}

impl DrawTool {
    pub const ALL: &'static [DrawTool] = &[
        DrawTool::Pen,
        DrawTool::Arrow,
        DrawTool::Line,
        DrawTool::Rect,
        DrawTool::Ellipse,
    ];

    pub fn label(self) -> &'static str {
        match self {
            DrawTool::Pen => "Pen",
            DrawTool::Arrow => "Arrow",
            DrawTool::Line => "Line",
            DrawTool::Rect => "Rect",
            DrawTool::Ellipse => "Ellipse",
        }
    }
}

/// Preset RGB colors for annotations.
pub const DRAW_COLOR_PRESETS: &[u32] = &[
    0xef4444, // Red
    0xf97316, // Orange
    0xfacc15, // Yellow
    0x22c55e, // Green
    0x3b82f6, // Blue
    0xffffff, // White
    0x181818, // Black
];

/// Preset stroke widths in logical pixels.
pub const STROKE_WIDTH_PRESETS: &[(&str, f32)] = &[("S", 2.0), ("M", 4.0), ("L", 7.0)];

/// Geometric shape of an annotation in `CapturedFrame` physical buffer coordinates.
#[derive(Debug, Clone, PartialEq)]
pub enum AnnotationShape {
    Freehand(Vec<(f32, f32)>),
    Line { start: (f32, f32), end: (f32, f32) },
    Arrow { start: (f32, f32), end: (f32, f32) },
    Rect { start: (f32, f32), end: (f32, f32) },
    Ellipse { start: (f32, f32), end: (f32, f32) },
}

/// A single drawn annotation on the captured frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Annotation {
    pub shape: AnnotationShape,
    /// RGB color (`0xRRGGBB`).
    pub color: u32,
    /// Stroke width in logical pixels.
    pub stroke_width: f32,
}

impl Annotation {
    /// Creates a new in-progress annotation at the given starting buffer point.
    pub fn start(tool: DrawTool, pt: (f32, f32), color: u32, stroke_width: f32) -> Self {
        let shape = match tool {
            DrawTool::Pen => AnnotationShape::Freehand(vec![pt]),
            DrawTool::Line => AnnotationShape::Line { start: pt, end: pt },
            DrawTool::Arrow => AnnotationShape::Arrow { start: pt, end: pt },
            DrawTool::Rect => AnnotationShape::Rect { start: pt, end: pt },
            DrawTool::Ellipse => AnnotationShape::Ellipse { start: pt, end: pt },
        };
        Self {
            shape,
            color,
            stroke_width,
        }
    }

    /// Updates the annotation with a new pointer position in buffer coordinates.
    pub fn update(&mut self, pt: (f32, f32)) {
        match &mut self.shape {
            AnnotationShape::Freehand(pts) => {
                if let Some(&(lx, ly)) = pts.last() {
                    let dx = pt.0 - lx;
                    let dy = pt.1 - ly;
                    if dx * dx + dy * dy >= 4.0 {
                        pts.push(pt);
                    }
                } else {
                    pts.push(pt);
                }
            }
            AnnotationShape::Line { end, .. }
            | AnnotationShape::Arrow { end, .. }
            | AnnotationShape::Rect { end, .. }
            | AnnotationShape::Ellipse { end, .. } => {
                *end = pt;
            }
        }
    }

    /// Returns true if the annotation has visible extent worth keeping.
    pub fn is_meaningful(&self) -> bool {
        match &self.shape {
            AnnotationShape::Freehand(pts) => !pts.is_empty(),
            AnnotationShape::Line { start, end } | AnnotationShape::Arrow { start, end } => {
                let dx = end.0 - start.0;
                let dy = end.1 - start.1;
                dx * dx + dy * dy >= 4.0
            }
            AnnotationShape::Rect { start, end } | AnnotationShape::Ellipse { start, end } => {
                (end.0 - start.0).abs() >= 2.0 && (end.1 - start.1).abs() >= 2.0
            }
        }
    }
}

/// Clamps a physical buffer coordinate to stay inside `rect`.
pub fn clamp_to_rect(pt: (f32, f32), rect: ScreenRect) -> (f32, f32) {
    let min_x = rect.x as f32;
    let min_y = rect.y as f32;
    let max_x = (rect.x + rect.width as i32) as f32;
    let max_y = (rect.y + rect.height as i32) as f32;
    (pt.0.clamp(min_x, max_x), pt.1.clamp(min_y, max_y))
}

/// Computes the two wing endpoints of an arrowhead at `end` pointing from `start` to `end`.
pub fn arrow_head_points(
    start: (f32, f32),
    end: (f32, f32),
    stroke_px: f32,
) -> Option<((f32, f32), (f32, f32))> {
    let dx = end.0 - start.0;
    let dy = end.1 - start.1;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 2.0 {
        return None;
    }

    let head_len = (stroke_px * 3.5).clamp(10.0, (len * 0.55).max(10.0)).min(len * 0.85);
    let theta = dy.atan2(dx);
    let wing_angle = 0.48_f32; // ~27.5 degrees

    let w1 = (
        end.0 - head_len * (theta - wing_angle).cos(),
        end.1 - head_len * (theta - wing_angle).sin(),
    );
    let w2 = (
        end.0 - head_len * (theta + wing_angle).cos(),
        end.1 - head_len * (theta + wing_angle).sin(),
    );
    Some((w1, w2))
}

/// Generates polygon vertices approximating an ellipse inscribed in `(start, end)`.
pub fn ellipse_points(start: (f32, f32), end: (f32, f32), segments: usize) -> Vec<(f32, f32)> {
    let cx = (start.0 + end.0) * 0.5;
    let cy = (start.1 + end.1) * 0.5;
    let rx = (end.0 - start.0).abs() * 0.5;
    let ry = (end.1 - start.1).abs() * 0.5;
    let count = segments.max(12);

    (0..count)
        .map(|i| {
            let angle = (i as f32 / count as f32) * std::f32::consts::TAU;
            (cx + rx * angle.cos(), cy + ry * angle.sin())
        })
        .collect()
}

/// Composites `annotations` onto a cropped `RgbaImage` whose top-left corresponds to `origin`
/// in `CapturedFrame` physical buffer coordinates.
pub fn composite_annotations(
    img: &mut RgbaImage,
    origin: (i32, i32),
    scale_factor: f32,
    annotations: &[Annotation],
) {
    if annotations.is_empty() {
        return;
    }

    let s = if scale_factor <= 0.0 { 1.0 } else { scale_factor };
    let ox = origin.0 as f32;
    let oy = origin.1 as f32;
    let shift = |pt: (f32, f32)| -> (f32, f32) { (pt.0 - ox, pt.1 - oy) };

    for ann in annotations {
        let rgb = [
            ((ann.color >> 16) & 0xff) as u8,
            ((ann.color >> 8) & 0xff) as u8,
            (ann.color & 0xff) as u8,
        ];
        let stroke_px = (ann.stroke_width * s).max(1.0);
        let radius = stroke_px * 0.5;

        match &ann.shape {
            AnnotationShape::Freehand(pts) => {
                if pts.len() == 1 {
                    let p = shift(pts[0]);
                    draw_thick_segment(img, p, p, radius, rgb);
                } else {
                    for win in pts.windows(2) {
                        draw_thick_segment(img, shift(win[0]), shift(win[1]), radius, rgb);
                    }
                }
            }
            AnnotationShape::Line { start, end } => {
                draw_thick_segment(img, shift(*start), shift(*end), radius, rgb);
            }
            AnnotationShape::Arrow { start, end } => {
                let p0 = shift(*start);
                let p1 = shift(*end);
                draw_thick_segment(img, p0, p1, radius, rgb);
                if let Some((w1, w2)) = arrow_head_points(p0, p1, stroke_px) {
                    draw_thick_segment(img, p1, w1, radius, rgb);
                    draw_thick_segment(img, p1, w2, radius, rgb);
                }
            }
            AnnotationShape::Rect { start, end } => {
                let p0 = shift(*start);
                let p1 = shift(*end);
                let min_x = p0.0.min(p1.0);
                let max_x = p0.0.max(p1.0);
                let min_y = p0.1.min(p1.1);
                let max_y = p0.1.max(p1.1);

                let tl = (min_x, min_y);
                let tr = (max_x, min_y);
                let br = (max_x, max_y);
                let bl = (min_x, max_y);

                draw_thick_segment(img, tl, tr, radius, rgb);
                draw_thick_segment(img, tr, br, radius, rgb);
                draw_thick_segment(img, br, bl, radius, rgb);
                draw_thick_segment(img, bl, tl, radius, rgb);
            }
            AnnotationShape::Ellipse { start, end } => {
                let pts = ellipse_points(shift(*start), shift(*end), 72);
                if pts.len() >= 2 {
                    for win in pts.windows(2) {
                        draw_thick_segment(img, win[0], win[1], radius, rgb);
                    }
                    draw_thick_segment(img, *pts.last().unwrap(), pts[0], radius, rgb);
                }
            }
        }
    }
}

fn draw_thick_segment(
    img: &mut RgbaImage,
    p0: (f32, f32),
    p1: (f32, f32),
    radius: f32,
    rgb: [u8; 3],
) {
    let w = img.width() as i32;
    let h = img.height() as i32;
    if w <= 0 || h <= 0 {
        return;
    }

    let pad = radius + 1.5;
    let min_x = (p0.0.min(p1.0) - pad).floor().max(0.0) as i32;
    let max_x = (p0.0.max(p1.0) + pad).ceil().min((w - 1) as f32) as i32;
    let min_y = (p0.1.min(p1.1) - pad).floor().max(0.0) as i32;
    let max_y = (p0.1.max(p1.1) + pad).ceil().min((h - 1) as f32) as i32;

    let vx = p1.0 - p0.0;
    let vy = p1.1 - p0.1;
    let len_sq = vx * vx + vy * vy;

    for y in min_y..=max_y {
        let py = y as f32 + 0.5;
        for x in min_x..=max_x {
            let px = x as f32 + 0.5;
            let t = if len_sq <= 1e-6 {
                0.0
            } else {
                (((px - p0.0) * vx + (py - p0.1) * vy) / len_sq).clamp(0.0, 1.0)
            };
            let proj_x = p0.0 + t * vx;
            let proj_y = p0.1 + t * vy;
            let dist = ((px - proj_x).powi(2) + (py - proj_y).powi(2)).sqrt();

            let alpha = (radius + 0.5 - dist).clamp(0.0, 1.0);
            if alpha <= 0.0 {
                continue;
            }

            let pixel = img.get_pixel_mut(x as u32, y as u32);
            let inv = 1.0 - alpha;
            pixel.0[0] = (rgb[0] as f32 * alpha + pixel.0[0] as f32 * inv).round() as u8;
            pixel.0[1] = (rgb[1] as f32 * alpha + pixel.0[1] as f32 * inv).round() as u8;
            pixel.0[2] = (rgb[2] as f32 * alpha + pixel.0[2] as f32 * inv).round() as u8;
            pixel.0[3] = 255;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn test_clamp_to_rect() {
        let rect = ScreenRect {
            x: 100,
            y: 50,
            width: 200,
            height: 150,
        };
        assert_eq!(clamp_to_rect((50.0, 10.0), rect), (100.0, 50.0));
        assert_eq!(clamp_to_rect((400.0, 300.0), rect), (300.0, 200.0));
        assert_eq!(clamp_to_rect((150.0, 100.0), rect), (150.0, 100.0));
    }

    #[test]
    fn test_composite_annotations_paints_pixels_in_crop() {
        let mut img = RgbaImage::from_pixel(100, 100, Rgba([0, 0, 0, 255]));
        let annotations = vec![
            Annotation {
                shape: AnnotationShape::Line {
                    start: (110.0, 250.0),
                    end: (190.0, 250.0),
                },
                color: 0xef4444,
                stroke_width: 4.0,
            },
            Annotation {
                shape: AnnotationShape::Rect {
                    start: (120.0, 220.0),
                    end: (180.0, 280.0),
                },
                color: 0x22c55e,
                stroke_width: 2.0,
            },
        ];

        composite_annotations(&mut img, (100, 200), 1.0, &annotations);

        // Midpoint of red line at (50, 50) in cropped coordinates
        let line_px = img.get_pixel(50, 50);
        assert!(line_px[0] > 200 && line_px[1] < 100 && line_px[2] < 100);

        // Top edge of green rect at (50, 20) in cropped coordinates
        let rect_px = img.get_pixel(50, 20);
        assert!(rect_px[1] > 180 && rect_px[0] < 100);

        // Untouched corner
        assert_eq!(img.get_pixel(0, 0), &Rgba([0, 0, 0, 255]));
    }

    #[test]
    fn test_arrow_head_points_nonempty_for_valid_segment() {
        let wings = arrow_head_points((10.0, 10.0), (110.0, 10.0), 4.0);
        assert!(wings.is_some());
        let (w1, w2) = wings.unwrap();
        assert!(w1.0 < 110.0 && w2.0 < 110.0);
        assert!(w1.1 > 10.0 && w2.1 < 10.0);
    }
}
