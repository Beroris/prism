//! Straight (not premultiplied) RGBA colour, written as `#rrggbb` or `#rrggbbaa` in settings.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Rgba {
        Rgba { r, g, b, a: 255 }
    }
    pub const fn with_alpha(self, a: u8) -> Rgba {
        Rgba { a, ..self }
    }

    pub const RED: Rgba = Rgba::rgb(0xF0, 0x3E, 0x3E);
    pub const ORANGE: Rgba = Rgba::rgb(0xFF, 0x8A, 0x1F);
    pub const YELLOW: Rgba = Rgba::rgb(0xFF, 0xD4, 0x3B);
    pub const GREEN: Rgba = Rgba::rgb(0x2F, 0xBF, 0x71);
    pub const BLUE: Rgba = Rgba::rgb(0x3B, 0x82, 0xF6);
    pub const PURPLE: Rgba = Rgba::rgb(0x9B, 0x5C, 0xF6);
    pub const BLACK: Rgba = Rgba::rgb(0x12, 0x12, 0x14);
    pub const WHITE: Rgba = Rgba::rgb(0xFF, 0xFF, 0xFF);

    pub const DEFAULT_PALETTE: [Rgba; 8] =
        [Rgba::RED, Rgba::ORANGE, Rgba::YELLOW, Rgba::GREEN, Rgba::BLUE, Rgba::PURPLE, Rgba::BLACK, Rgba::WHITE];

    /// Relative luminance, 0 to 1, for choosing readable text on top of this colour.
    pub fn luminance(self) -> f32 {
        let lin = |c: u8| {
            let c = c as f32 / 255.0;
            if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * lin(self.r) + 0.7152 * lin(self.g) + 0.0722 * lin(self.b)
    }

    /// Black or white, whichever reads better on this colour.
    pub fn contrasting(self) -> Rgba {
        if self.luminance() > 0.4 { Rgba::BLACK } else { Rgba::WHITE }
    }

    pub fn to_hex(self) -> String {
        if self.a == 255 {
            format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
        }
    }

    pub fn parse(s: &str) -> Option<Rgba> {
        let h = s.trim().strip_prefix('#')?;
        if !h.is_ascii() {
            return None;
        }
        let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
        match h.len() {
            6 => Some(Rgba::rgb(byte(0)?, byte(2)?, byte(4)?)),
            8 => Some(Rgba { r: byte(0)?, g: byte(2)?, b: byte(4)?, a: byte(6)? }),
            _ => None,
        }
    }
}

impl Serialize for Rgba {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Rgba {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Rgba::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("not a colour: {s:?}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip() {
        for c in Rgba::DEFAULT_PALETTE {
            assert_eq!(Rgba::parse(&c.to_hex()), Some(c));
        }
        let t = Rgba::RED.with_alpha(0x80);
        assert_eq!(t.to_hex(), "#f03e3e80");
        assert_eq!(Rgba::parse("#f03e3e80"), Some(t));
        assert_eq!(Rgba::parse("f03e3e"), None);
        assert_eq!(Rgba::parse("#zzzzzz"), None);
        assert_eq!(Rgba::parse("#ééé"), None);
    }

    #[test]
    fn contrast_picks_readable() {
        assert_eq!(Rgba::YELLOW.contrasting(), Rgba::BLACK);
        assert_eq!(Rgba::BLUE.contrasting(), Rgba::WHITE);
        assert_eq!(Rgba::RED.contrasting(), Rgba::WHITE);
    }
}
