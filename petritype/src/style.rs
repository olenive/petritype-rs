//! How a net's tokens and places look: data a viewer draws, declared beside
//! the net (DESIGN §18, 2026-10-16, net-declared controls).
//!
//! A **token style** is what a token looks like, derived from its value by a
//! function the net's author writes, as a token's [keys](crate::Keys) are:
//! [`NetBuilder::style`](crate::NetBuilder::style) gives a place one, and
//! [`NetBuilder::styled`](crate::NetBuilder::styled) is the one line that
//! opts a place in when its token type implements [`TokenStyle`].
//! [`Net::style`](crate::Net::style) reads one token's [`Style`]. In this
//! round a style is a colour; a glyph and an image join it later, which is
//! why [`Style`] is `#[non_exhaustive]`.
//!
//! A **place style** is a rule the viewer evaluates:
//! [`PlaceStyle::fill_token`] asks for the place to be filled with the
//! colour of the one token it holds, while it holds exactly one that has a
//! colour. The net only carries the rule, on
//! [`PlaceInfo::style`](crate::PlaceInfo::style) and the
//! [`Topology`](crate::Topology); the colour is the token's own.
//!
//! ```
//! use petritype::NetBuilder;
//! use petritype::style::{PlaceStyle, Rgb, Style, TokenStyle};
//!
//! #[derive(Debug, Clone, PartialEq)]
//! enum Lamp { Red, Green }
//!
//! impl TokenStyle for Lamp {
//!     fn style(&self) -> Style {
//!         Style::default().with_colour(match self {
//!             Lamp::Red => Rgb(0xff, 0x63, 0x47),
//!             Lamp::Green => Rgb(0x32, 0xcd, 0x32),
//!         })
//!     }
//! }
//!
//! let mut b = NetBuilder::new();
//! let lamp = b.place::<Lamp>("Lamp");
//! let plain = b.place::<u32>("Plain");
//! b.styled(lamp);
//! b.place_style(lamp, PlaceStyle::fill_token());
//! b.seed(lamp, [Lamp::Green]);
//! b.seed(plain, [7]);
//! let net = b.build()?;
//!
//! let (green, _) = net.tokens(lamp.id()).next().unwrap();
//! let colour = net.style(lamp.id(), green).and_then(|style| style.colour);
//! assert_eq!(colour.map(|rgb| rgb.to_string()), Some("#32cd32".to_owned()));
//!
//! let (seven, _) = net.tokens(plain.id()).next().unwrap();
//! assert_eq!(net.style(plain.id(), seven), None, "a place with no style");
//! assert_eq!(net.topology().places[0].style, Some(PlaceStyle::fill_token()));
//! # Ok::<(), petritype::BuildError>(())
//! ```

use std::any::Any;
use std::fmt;
use std::sync::Arc;

/// A colour, red, green and blue, one byte each.
///
/// [`Display`](fmt::Display) writes it as CSS does, `#rrggbb` in lower
/// case, which is the protocol's `color` (PROTOCOL.md §5.3), and
/// [`hex`](Rgb::hex) reads that form back.
///
/// ```
/// use petritype::style::Rgb;
///
/// let tomato = Rgb(0xff, 0x63, 0x47);
/// assert_eq!(tomato.to_string(), "#ff6347");
/// assert_eq!(Rgb::hex("#FF6347"), Some(tomato));
/// assert_eq!(Rgb::hex("ff6347"), None, "the # is required");
/// assert_eq!(Rgb::hex("#f63"), None, "six digits, not three");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// Reads `#rrggbb`, either case; `None` for anything else.
    pub fn hex(text: &str) -> Option<Rgb> {
        let digits = text.strip_prefix('#')?;
        if digits.len() != 6 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        let byte = |at: usize| u8::from_str_radix(&digits[at..at + 2], 16).ok();
        Some(Rgb(byte(0)?, byte(2)?, byte(4)?))
    }
}

impl fmt::Display for Rgb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }
}

/// What one token looks like: its colour, if it has one.
///
/// `#[non_exhaustive]`, because a glyph and an image join the colour in a
/// later round: build one from [`Style::default`] and
/// [`with_colour`](Style::with_colour). The default is no style at all,
/// which a viewer draws as it always has.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Style {
    /// The token's colour, which the protocol carries as `color`
    /// (PROTOCOL.md §5.3).
    pub colour: Option<Rgb>,
}

impl Style {
    /// This style with its colour set.
    pub fn with_colour(mut self, colour: Rgb) -> Style {
        self.colour = Some(colour);
        self
    }
}

/// A token type that knows what it looks like.
///
/// Implementing it does nothing by itself: places have no trait bound and
/// Rust no specialisation to find it with, so a place opts in with
/// [`NetBuilder::styled`](crate::NetBuilder::styled), which is
/// [`NetBuilder::style`](crate::NetBuilder::style) with
/// [`TokenStyle::style`] as the function.
pub trait TokenStyle {
    /// This token's style.
    fn style(&self) -> Style;
}

/// What a place looks like, as a rule the viewer evaluates (PROTOCOL.md
/// §5.1, `places[].style`): see [`Fill`].
///
/// ```
/// use petritype::style::{Fill, PlaceStyle};
///
/// assert_eq!(PlaceStyle::fill_token().fill, Fill::Token);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[non_exhaustive]
pub struct PlaceStyle {
    /// How the place is filled.
    pub fill: Fill,
}

impl PlaceStyle {
    /// The place filled with its token's colour: [`Fill::Token`].
    pub fn fill_token() -> PlaceStyle {
        PlaceStyle { fill: Fill::Token }
    }
}

/// How a [`PlaceStyle`] fills its place. With the `serde` feature it is
/// written in snake case, so a place style is `{"fill": "token"}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
#[non_exhaustive]
pub enum Fill {
    /// While the place holds exactly one token and that token has a colour,
    /// the place is filled with it; otherwise the viewer's own fill. A
    /// firing's highlight wins while it lasts.
    Token,
}

/// Derives one token's style. See
/// [`NetBuilder::style`](crate::NetBuilder::style).
///
/// A closure the author wrote, so not a plain `fn` pointer: boxed, in an
/// `Arc` so that [`PlaceMeta`](crate::place::PlaceMeta) stays `Clone` and a
/// [`Snapshot`](crate::Snapshot) shares it, as the keys' extractor is.
pub(crate) type StyleFn = Arc<dyn Fn(&dyn Any) -> Style + Send + Sync>;

/// The style function for a place of `T`: `f`, wrapped so that it takes an
/// erased token.
pub(crate) fn style_fn<T: 'static>(f: impl Fn(&T) -> Style + Send + Sync + 'static) -> StyleFn {
    Arc::new(move |value: &dyn Any| {
        let value = value
            .downcast_ref::<T>()
            .expect("token in a place is always of the place's type");
        f(value)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_reads_what_display_writes() {
        for rgb in [Rgb(0, 0, 0), Rgb(255, 255, 255), Rgb(0x32, 0xcd, 0x32)] {
            assert_eq!(Rgb::hex(&rgb.to_string()), Some(rgb));
        }
    }

    #[test]
    fn hex_refuses_what_is_not_six_hex_digits() {
        for text in ["", "#", "#12345", "#1234567", "#12345g", "123456", "#ü2345"] {
            assert_eq!(Rgb::hex(text), None, "{text:?}");
        }
    }

    #[test]
    fn the_default_style_has_no_colour() {
        assert_eq!(Style::default().colour, None);
        assert_eq!(
            Style::default().with_colour(Rgb(1, 2, 3)).colour,
            Some(Rgb(1, 2, 3))
        );
    }
}
