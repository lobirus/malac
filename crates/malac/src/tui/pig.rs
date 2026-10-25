//! The welcome-screen mascot: palette-indexed pixel art drawn with half
//! blocks, so each terminal cell shows two vertical pixels.

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

/// Pixel colors by key; `.` is transparent.
const PALETTE: [(char, u32); 9] = [
    ('h', 0xffdedb), // highlight
    ('l', 0xf8b5b8), // light skin
    ('p', 0xee929f), // base skin
    ('m', 0xd77386), // mid shade
    ('s', 0xb0566c), // shadow
    ('d', 0x7d3a50), // deep shadow and contour
    ('e', 0xe2647a), // inner ear
    ('k', 0x2b1b1f), // eyes and nostrils
    ('w', 0xffffff), // eye highlight
];

const PIXELS: [&str; 36] = [
    "..mmmmm..............................mmmmm..",
    ".mhhhllmmm........................mmmlppppm.",
    ".mhlllllllmm....................mmppppppppm.",
    ".mllssssllllmm................mmppppssssppm.",
    ".mleeeeeeeelllmm............mmpppeeeeeeeepm.",
    ".mleeeeeeeeeelllmm........mmpppeeeeeeeeeepm.",
    "..mleeeeeeeeeellllm......mppppeeeeeeeeeepm..",
    "..mleeeeeeeeeeeellm.msss.mppeeeeeeeeeeeepm..",
    "..mleeeeeeeeeeepmmmmpmmmsmmsmeeeeeeeeeeepm..",
    "...mleeeeeeeepmpppppppppppmmmsseeeeeeeepm...",
    "...mlmmmmmmmmppppplppppppppmmmssmmmmmmmpm...",
    "....mlmmmmmmpppllllllllpppppmmmssmmmmmpm....",
    "....mllsssmpplllhhhlllllppppmmmssdsssppm....",
    ".....mlllmpppllhhhhhlllllppppmmmssdpppm.....",
    "......mmmmpppmmphhhllllllpppmssmmssdmm......",
    "........spppllllllllllllpppppmmmmssd........",
    ".......smppplwklllllllllppppmwkmmmssd.......",
    ".......smppplkkllllllllpppppmkkmmmssd.......",
    ".......smppppkklllllpppppppmmkkmmmssd.......",
    "......smppllllppppppppppppmmmmmmmmmssd......",
    "......smplllllpppppppppppmmmmmmmppmssd......",
    "......smpllllpppppmmmmmmssmmmmmpppmssd......",
    "......smppllppppmmhhhhhlllsssmmmpmmssd......",
    "......smpppppppmhhllllllllppssmmmmmssd......",
    "......smmpppppmhllllllllllppmssmmmmssd......",
    "......smmmppppmhllkkllllkkppmssmmmmssd......",
    "......dsmmmpppmlllkklllpkkppmssmmmmssd......",
    "......dsmmmmppslllkklpppkkpmmdsmmmmssd......",
    ".......dsmmmmpmsplllppppppmmdssmmmssd.......",
    ".......dssmmmmmmssmpppppmmddsssmmsssd.......",
    "........ddssmmmmmsssssdddddsssssssdd........",
    ".........dssmmmmmssssssssssssssssdd.........",
    "...........ddssmmmmmmmmmmmsssssdd...........",
    "..............dddssssssssssddd..............",
    ".................dddddddddd.................",
    "............................................",
];

pub const WIDTH: usize = 44;
pub const HEIGHT: usize = PIXELS.len() / 2;

fn rgb(key: char) -> Option<(u8, u8, u8)> {
    let (_, hex) = PALETTE.iter().find(|(k, _)| *k == key)?;
    Some(((hex >> 16) as u8, (hex >> 8) as u8, *hex as u8))
}

fn color((r, g, b): (u8, u8, u8), truecolor: bool) -> Color {
    if truecolor {
        Color::Rgb(r, g, b)
    } else {
        Color::Indexed(xterm256(r, g, b))
    }
}

/// The nearest color in the xterm 256-color cube or grey ramp.
pub fn xterm256(r: u8, g: u8, b: u8) -> u8 {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let level = |v: u8| {
        (0..6)
            .min_by_key(|&i| (i32::from(LEVELS[i]) - i32::from(v)).abs())
            .unwrap_or(0)
    };
    let distance = |(r2, g2, b2): (u8, u8, u8)| {
        let d = |a: u8, b: u8| (i32::from(a) - i32::from(b)).pow(2);
        d(r, r2) + d(g, g2) + d(b, b2)
    };
    let (ri, gi, bi) = (level(r), level(g), level(b));
    let cube = (LEVELS[ri], LEVELS[gi], LEVELS[bi]);
    let average = (u32::from(r) + u32::from(g) + u32::from(b)) / 3;
    let step = (average.saturating_sub(3) / 10).min(23) as u8;
    let grey = 8 + 10 * step;
    if distance((grey, grey, grey)) < distance(cube) {
        232 + step
    } else {
        16 + 36 * ri as u8 + 6 * gi as u8 + bi as u8
    }
}

/// The pig as lines of half-block cells.
pub fn lines(truecolor: bool) -> Vec<Line<'static>> {
    let rows: Vec<Vec<char>> = PIXELS.iter().map(|r| r.chars().collect()).collect();
    rows.chunks(2)
        .map(|pair| {
            let spans = (0..WIDTH).map(|x| {
                let top = rgb(pair[0][x]).map(|c| color(c, truecolor));
                let bottom = pair
                    .get(1)
                    .and_then(|r| rgb(r[x]))
                    .map(|c| color(c, truecolor));
                match (top, bottom) {
                    (None, None) => Span::raw(" "),
                    (Some(top), None) => Span::styled("▀", Style::new().fg(top)),
                    (None, Some(bottom)) => Span::styled("▄", Style::new().fg(bottom)),
                    (Some(top), Some(bottom)) => Span::styled("▀", Style::new().fg(top).bg(bottom)),
                }
            });
            Line::from(spans.collect::<Vec<_>>())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_map_is_well_formed() {
        assert_eq!(PIXELS.len() % 2, 0);
        for row in PIXELS {
            assert_eq!(row.chars().count(), WIDTH);
            assert!(row.chars().all(|c| c == '.' || rgb(c).is_some()), "{row}");
        }
        let lines = lines(true);
        assert_eq!(lines.len(), HEIGHT);
        assert!(lines.iter().all(|l| l.width() == WIDTH));
    }

    #[test]
    fn maps_to_the_256_color_palette() {
        assert_eq!(xterm256(255, 0, 0), 196);
        assert_eq!(xterm256(0, 0, 0), 16);
        assert_eq!(xterm256(128, 128, 128), 244);
        assert_eq!(xterm256(0xee, 0x92, 0x9f), 211);
    }
}
