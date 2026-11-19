//! Line icons drawn from SVG path data (the design's 24×24 stroke icons), for the few icons
//! Segoe Fluent Icons doesn't have (service marks) or draws differently from the design.
//!
//! Supports the path commands those icons use: `M L H V C S A Z`, absolute and relative.
//! Curves and arcs are flattened into short segments.

use eframe::egui::{epaint::PathShape, epaint::PathStroke, Color32, Painter, Pos2, Rect, Stroke};

/// One icon: path data and circles (`cx, cy, r`, filled when `fill`), on a 24×24 grid.
pub struct Icon {
    pub paths: &'static [&'static str],
    pub circles: &'static [(f32, f32, f32, bool)],
}

pub const GITHUB: Icon = Icon {
    paths: &["M9 19c-4 1.5-4-2-6-2.5M15 21v-3.5a3 3 0 0 0-.9-2.4c3-.3 6-1.5 6-6.6a5 5 0 0 0-1.4-3.6 4.7 4.7 0 0 0-.1-3.6s-1.1-.3-3.7 1.4a12.6 12.6 0 0 0-6.6 0C5.7 1 4.6 1.3 4.6 1.3a4.7 4.7 0 0 0-.1 3.6A5 5 0 0 0 3 8.5c0 5.1 3 6.3 6 6.6a3 3 0 0 0-.9 2.4V21"],
    circles: &[],
};
pub const GITLAB: Icon = Icon {
    paths: &["m12 21-9-7 2.5-10 3 7h7l3-7L21 14z"],
    circles: &[],
};
pub const ACCENT: Icon = Icon {
    paths: &[],
    circles: &[(12.0, 12.0, 9.0, false), (12.0, 12.0, 4.0, true)],
};
pub const CODE: Icon = Icon {
    paths: &["m8 7-5 5 5 5M16 7l5 5-5 5"],
    circles: &[],
};
pub const DENSITY: Icon = Icon {
    paths: &["M4 6h16M4 12h16M4 18h16"],
    circles: &[],
};
pub const PERSON: Icon = Icon {
    paths: &["M4 21a8 8 0 0 1 16 0"],
    circles: &[(12.0, 8.0, 4.0, false)],
};
pub const SYNC: Icon = Icon {
    paths: &[
        "M21 12a9 9 0 0 1-15.5 6.2L3 16",
        "M3 21v-5h5",
        "M3 12a9 9 0 0 1 15.5-6.2L21 8",
        "M21 3v5h-5",
    ],
    circles: &[],
};
pub const PULL: Icon = Icon {
    paths: &["M12 5v14M19 12l-7 7-7-7"],
    circles: &[],
};
pub const BRANCH: Icon = Icon {
    paths: &["M6 3v12", "M18 9a9 9 0 0 1-9 9"],
    circles: &[(18.0, 6.0, 3.0, false), (6.0, 18.0, 3.0, false)],
};
pub const EDIT: Icon = Icon {
    paths: &["M12 20h9", "M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z"],
    circles: &[],
};
pub const TERMINAL: Icon = Icon {
    paths: &["M5 4h14a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2z", "m7 9 3 3-3 3M13 15h4"],
    circles: &[],
};
pub const SLIDERS: Icon = Icon {
    paths: &["M4 6h9M19 6h1M4 12h3M13 12h7M4 18h11M21 18h-1"],
    circles: &[(16.0, 6.0, 2.2, false), (10.0, 12.0, 2.2, false), (18.0, 18.0, 2.2, false)],
};
pub const GIT: Icon = Icon {
    paths: &["M14 3H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9z", "M14 3v6h6", "M9 15h6M12 12v6"],
    circles: &[],
};
pub const MONITOR: Icon = Icon {
    paths: &["M5 4h14a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2z", "M8 21h8M12 17v4"],
    circles: &[],
};
pub const LINE_ENDINGS: Icon = Icon {
    paths: &["M20 5v7a3 3 0 0 1-3 3H5", "m9 11-4 4 4 4"],
    circles: &[],
};
pub const STORAGE: Icon = Icon {
    paths: &[
        "M4 7c0-1.7 3.6-3 8-3s8 1.3 8 3-3.6 3-8 3-8-1.3-8-3z",
        "M4 7v10c0 1.7 3.6 3 8 3s8-1.3 8-3V7",
        "M4 12c0 1.7 3.6 3 8 3s8-1.3 8-3",
    ],
    circles: &[],
};
pub const PULSE: Icon = Icon {
    paths: &["M3 12h4l3-8 4 16 3-8h4"],
    circles: &[],
};
pub const FOLDER: Icon = Icon {
    paths: &["M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"],
    circles: &[],
};

/// Draw `icon` in `rect` (a square; the 24-unit grid is scaled to it) with a `width`-point
/// stroke (in the 24-unit grid, like the SVG's `stroke-width`).
pub fn draw(painter: &Painter, rect: Rect, icon: &Icon, width: f32, color: Color32) {
    let scale = rect.width() / 24.0;
    let map = |x: f32, y: f32| Pos2::new(rect.left() + x * scale, rect.top() + y * scale);
    let stroke = Stroke::new(width * scale, color);
    for d in icon.paths {
        for (points, closed) in flatten(d) {
            let points: Vec<Pos2> = points.into_iter().map(|(x, y)| map(x, y)).collect();
            if points.len() < 2 {
                continue;
            }
            // Round joins and caps, as the design's icons have.
            for point in &points {
                painter.circle_filled(*point, stroke.width / 2.0, color);
            }
            painter.add(if closed {
                PathShape::closed_line(points, PathStroke::from(stroke))
            } else {
                PathShape::line(points, PathStroke::from(stroke))
            });
        }
    }
    for &(cx, cy, r, fill) in icon.circles {
        if fill {
            painter.circle_filled(map(cx, cy), r * scale, color);
        } else {
            painter.circle_stroke(map(cx, cy), r * scale, stroke);
        }
    }
}

type Subpath = (Vec<(f32, f32)>, bool);

/// Path data as polylines (and whether each is closed).
fn flatten(d: &str) -> Vec<Subpath> {
    let tokens = tokenize(d);
    let mut out: Vec<Subpath> = Vec::new();
    let mut current: Vec<(f32, f32)> = Vec::new();
    let (mut x, mut y) = (0.0f32, 0.0f32);
    let (mut start_x, mut start_y) = (0.0, 0.0);
    // The second control point of the last cubic (for `S`).
    let mut last_control: Option<(f32, f32)> = None;
    let mut index = 0;
    let mut command = 'M';
    let number = |index: &mut usize| -> Option<f32> {
        match tokens.get(*index) {
            Some(Token::Number(value)) => {
                *index += 1;
                Some(*value)
            }
            _ => None,
        }
    };
    while index < tokens.len() {
        if let Token::Command(c) = tokens[index] {
            command = c;
            index += 1;
            if c == 'z' || c == 'Z' {
                if !current.is_empty() {
                    out.push((std::mem::take(&mut current), true));
                }
                x = start_x;
                y = start_y;
                last_control = None;
                continue;
            }
        }
        let relative = command.is_ascii_lowercase();
        let (ox, oy) = if relative { (x, y) } else { (0.0, 0.0) };
        match command.to_ascii_uppercase() {
            'M' => {
                let (Some(nx), Some(ny)) = (number(&mut index), number(&mut index)) else {
                    break;
                };
                if current.len() > 1 {
                    out.push((std::mem::take(&mut current), false));
                }
                current.clear();
                x = ox + nx;
                y = oy + ny;
                start_x = x;
                start_y = y;
                current.push((x, y));
                // Further pairs after a move are line-tos.
                command = if relative { 'l' } else { 'L' };
                last_control = None;
            }
            'L' => {
                let (Some(nx), Some(ny)) = (number(&mut index), number(&mut index)) else {
                    break;
                };
                x = ox + nx;
                y = oy + ny;
                current.push((x, y));
                last_control = None;
            }
            'H' => {
                let Some(nx) = number(&mut index) else { break };
                x = ox + nx;
                current.push((x, y));
                last_control = None;
            }
            'V' => {
                let Some(ny) = number(&mut index) else { break };
                y = oy + ny;
                current.push((x, y));
                last_control = None;
            }
            'C' | 'S' => {
                let smooth = command.eq_ignore_ascii_case(&'S');
                let first = if smooth {
                    last_control.map_or((x, y), |(cx, cy)| (2.0 * x - cx, 2.0 * y - cy))
                } else {
                    let (Some(ax), Some(ay)) = (number(&mut index), number(&mut index)) else {
                        break;
                    };
                    (ox + ax, oy + ay)
                };
                let (Some(bx), Some(by), Some(ex), Some(ey)) = (
                    number(&mut index),
                    number(&mut index),
                    number(&mut index),
                    number(&mut index),
                ) else {
                    break;
                };
                let second = (ox + bx, oy + by);
                let end = (ox + ex, oy + ey);
                for step in 1..=12 {
                    let t = step as f32 / 12.0;
                    let u = 1.0 - t;
                    let point = |a: f32, b: f32, c: f32, d: f32| {
                        u * u * u * a + 3.0 * u * u * t * b + 3.0 * u * t * t * c + t * t * t * d
                    };
                    current.push((
                        point(x, first.0, second.0, end.0),
                        point(y, first.1, second.1, end.1),
                    ));
                }
                last_control = Some(second);
                (x, y) = end;
            }
            'A' => {
                let values: Vec<f32> = (0..7).filter_map(|_| number(&mut index)).collect();
                let [rx, ry, rotation, large, sweep, ex, ey] = values[..] else {
                    break;
                };
                let end = (ox + ex, oy + ey);
                arc(&mut current, (x, y), end, (rx, ry), rotation, large != 0.0, sweep != 0.0);
                (x, y) = end;
                last_control = None;
            }
            _ => break,
        }
    }
    if current.len() > 1 {
        out.push((current, false));
    }
    out
}

/// SVG's endpoint arc as points (SVG 1.1 implementation notes, F.6.5).
fn arc(
    points: &mut Vec<(f32, f32)>,
    (x1, y1): (f32, f32),
    (x2, y2): (f32, f32),
    (rx, ry): (f32, f32),
    rotation: f32,
    large: bool,
    sweep: bool,
) {
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx < 1e-4 || ry < 1e-4 {
        points.push((x2, y2));
        return;
    }
    let phi = rotation.to_radians();
    let (sin, cos) = phi.sin_cos();
    let dx = (x1 - x2) / 2.0;
    let dy = (y1 - y2) / 2.0;
    let x1p = cos * dx + sin * dy;
    let y1p = -sin * dx + cos * dy;
    let lambda = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
    if lambda > 1.0 {
        rx *= lambda.sqrt();
        ry *= lambda.sqrt();
    }
    let numerator = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p;
    let denominator = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
    let mut factor = (numerator / denominator).max(0.0).sqrt();
    if large == sweep {
        factor = -factor;
    }
    let cxp = factor * rx * y1p / ry;
    let cyp = -factor * ry * x1p / rx;
    let cx = cos * cxp - sin * cyp + (x1 + x2) / 2.0;
    let cy = sin * cxp + cos * cyp + (y1 + y2) / 2.0;
    let angle = |ux: f32, uy: f32, vx: f32, vy: f32| {
        let sign = if ux * vy - uy * vx < 0.0 { -1.0 } else { 1.0 };
        let dot = (ux * vx + uy * vy) / ((ux * ux + uy * uy).sqrt() * (vx * vx + vy * vy).sqrt());
        sign * dot.clamp(-1.0, 1.0).acos()
    };
    let theta = angle(1.0, 0.0, (x1p - cxp) / rx, (y1p - cyp) / ry);
    let mut delta = angle(
        (x1p - cxp) / rx,
        (y1p - cyp) / ry,
        (-x1p - cxp) / rx,
        (-y1p - cyp) / ry,
    );
    if !sweep && delta > 0.0 {
        delta -= std::f32::consts::TAU;
    } else if sweep && delta < 0.0 {
        delta += std::f32::consts::TAU;
    }
    let steps = ((delta.abs() / 0.2).ceil() as usize).max(2);
    for step in 1..=steps {
        let t = theta + delta * step as f32 / steps as f32;
        let (s, c) = t.sin_cos();
        points.push((
            cx + cos * rx * c - sin * ry * s,
            cy + sin * rx * c + cos * ry * s,
        ));
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Token {
    Command(char),
    Number(f32),
}

fn tokenize(d: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = d.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_ascii_alphabetic() {
            tokens.push(Token::Command(c));
            i += 1;
        } else if c.is_ascii_digit() || c == '-' || c == '.' || c == '+' {
            let start = i;
            i += 1;
            let mut dot = c == '.';
            while i < chars.len() {
                let next = chars[i];
                if next.is_ascii_digit() {
                    i += 1;
                } else if next == '.' && !dot {
                    dot = true;
                    i += 1;
                } else if (next == 'e' || next == 'E')
                    && chars.get(i + 1).is_some_and(|c| c.is_ascii_digit() || *c == '-')
                {
                    i += 2;
                } else {
                    break;
                }
            }
            let text: String = chars[start..i].iter().collect();
            if let Ok(value) = text.parse() {
                tokens.push(Token::Number(value));
            }
        } else {
            i += 1;
        }
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_numbers_split_at_signs_and_second_dots() {
        assert_eq!(
            tokenize("a3 3 0 0 0-.9-2.4"),
            vec![
                Token::Command('a'),
                Token::Number(3.0),
                Token::Number(3.0),
                Token::Number(0.0),
                Token::Number(0.0),
                Token::Number(0.0),
                Token::Number(-0.9),
                Token::Number(-2.4),
            ]
        );
        assert_eq!(
            tokenize("M1.5.5"),
            vec![Token::Command('M'), Token::Number(1.5), Token::Number(0.5)]
        );
    }

    #[test]
    fn relative_lines_and_close() {
        let paths = flatten("m12 21-9-7 2.5-10z");
        assert_eq!(paths.len(), 1);
        assert!(paths[0].1);
        assert_eq!(paths[0].0, vec![(12.0, 21.0), (3.0, 14.0), (5.5, 4.0)]);
    }

    #[test]
    fn arcs_end_where_they_should() {
        let paths = flatten("M21 12a9 9 0 0 1-15.5 6.2");
        let last = *paths[0].0.last().unwrap();
        assert!((last.0 - 5.5).abs() < 0.01 && (last.1 - 18.2).abs() < 0.01);
    }
}
