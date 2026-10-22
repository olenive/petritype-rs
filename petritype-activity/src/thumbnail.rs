//! [`Thumbnail`]: a colour per cell, from the activity, the grid, the hue
//! mode and a [`Palette`].

use petritype::style::Rgb;
use petritype_layout::LayoutNode;

use crate::grid::Grid;
use crate::model::Activity;
use crate::settings::HueMode;

/// The colours a thumbnail is drawn with, for one theme: plain [`Rgb`]
/// values the caller supplies.
///
/// [`LIGHT`](Palette::LIGHT) and [`DARK`](Palette::DARK) mirror the Bevy
/// viewer's two themes: places in its place blues, transitions in its
/// fireable greens, its running and failed colours and its page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Palette {
    /// An empty cell, and the page behind the thumbnail.
    pub page: Rgb,
    /// A place at glow 0.
    pub place_dim: Rgb,
    /// A place at glow 1, under [`HueMode::Kind`].
    pub place_bright: Rgb,
    /// A transition at glow 0.
    pub transition_dim: Rgb,
    /// A transition at glow 1.
    pub transition_bright: Rgb,
    /// A place at glow 1 draining at full flow, under [`HueMode::Flow`].
    pub cold: Rgb,
    /// A place at glow 1 with no flow, under [`HueMode::Flow`].
    pub neutral: Rgb,
    /// A place at glow 1 filling at full flow, under [`HueMode::Flow`].
    pub warm: Rgb,
    /// A transition whose failure level is 1.
    pub failed: Rgb,
    /// A transition in flight.
    pub running: Rgb,
}

impl Palette {
    /// The light theme: dark ink on a pale page. A glow deepens a cell.
    pub const LIGHT: Palette = Palette {
        page: Rgb(250, 250, 250),
        place_dim: Rgb(214, 236, 255),
        place_bright: Rgb(43, 108, 176),
        transition_dim: Rgb(154, 230, 180),
        transition_bright: Rgb(39, 103, 73),
        cold: Rgb(43, 108, 176),
        neutral: Rgb(160, 174, 192),
        warm: Rgb(192, 86, 33),
        failed: Rgb(252, 129, 129),
        running: Rgb(246, 224, 94),
    };

    /// The dark theme: pale ink on a dark page. A glow lightens a cell.
    pub const DARK: Palette = Palette {
        page: Rgb(26, 27, 31),
        place_dim: Rgb(30, 58, 86),
        place_bright: Rgb(99, 179, 237),
        transition_dim: Rgb(39, 103, 73),
        transition_bright: Rgb(154, 230, 180),
        cold: Rgb(99, 179, 237),
        neutral: Rgb(160, 174, 192),
        warm: Rgb(246, 173, 85),
        failed: Rgb(197, 48, 48),
        running: Rgb(183, 121, 31),
    };

    /// A place's colour: its dim colour toward its bright one by `glow`.
    /// Under [`HueMode::Flow`] the bright end runs from
    /// [`neutral`](Palette::neutral) toward [`warm`](Palette::warm) as
    /// `flow` rises to 1 and toward [`cold`](Palette::cold) as it falls to
    /// -1; the glow is unchanged. At glow 0 a place is its dim colour in
    /// either mode, so a still thumbnail shows the net's shape.
    pub fn place(&self, glow: f32, flow: f32, hue: HueMode) -> Rgb {
        let bright = match hue {
            HueMode::Kind => self.place_bright,
            HueMode::Flow if flow >= 0.0 => mix(self.neutral, self.warm, flow),
            HueMode::Flow => mix(self.neutral, self.cold, -flow),
        };
        mix(self.place_dim, bright, glow)
    }

    /// A transition's colour: the [`running`](Palette::running) colour in
    /// flight; otherwise its dim colour toward its bright one by `glow`,
    /// and that toward the [`failed`](Palette::failed) colour by `failure`.
    /// Transitions keep their hue under either [`HueMode`], one outside the
    /// flow's range.
    pub fn transition(&self, glow: f32, failure: f32, in_flight: bool) -> Rgb {
        if in_flight {
            return self.running;
        }
        let lit = mix(self.transition_dim, self.transition_bright, glow);
        mix(lit, self.failed, failure)
    }
}

/// `a` toward `b` by `t`, clamped to 0 to 1, each channel rounded.
fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) };
    let channel = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
    Rgb(channel(a.0, b.0), channel(a.1, b.1), channel(a.2, b.2))
}

/// The thumbnail at one moment: a colour for every position of a [`Grid`],
/// empty ones in the page colour. A pure function of its inputs, and a
/// value any renderer can draw, a texture a pixel per cell or the
/// [`svg`](Thumbnail::svg).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thumbnail {
    columns: usize,
    rows: usize,
    /// Row-major, `columns * rows` long.
    colours: Vec<Rgb>,
}

impl Thumbnail {
    /// The thumbnail of `activity` on `grid`, tinted by `hue`, in
    /// `palette`.
    ///
    /// A place is coloured by its [glow](Activity::glow) and flow, a
    /// transition by its glow, its failure level (dimmed with the whole by
    /// the [freshness](Activity::freshness)) and whether it is in flight;
    /// see [`Palette::place`] and [`Palette::transition`]. A node the
    /// activity does not know is drawn at rest.
    pub fn new(activity: &Activity, grid: &Grid, hue: HueMode, palette: &Palette) -> Thumbnail {
        Thumbnail::paint(grid, palette, |node| match node {
            LayoutNode::Place(id) => {
                let flow = activity.place(id).map_or(0.0, |p| p.flow);
                palette.place(activity.glow(node), flow, hue)
            }
            LayoutNode::Transition(id) => {
                let t = activity.transition(id).unwrap_or_default();
                let failure = t.failure * activity.freshness();
                palette.transition(activity.glow(node), failure, t.in_flight)
            }
        })
    }

    /// The thumbnail with no activity at all: every cell its kind's dim
    /// colour, the net's icon.
    pub fn icon(grid: &Grid, palette: &Palette) -> Thumbnail {
        Thumbnail::paint(grid, palette, |node| match node {
            LayoutNode::Place(_) => palette.place_dim,
            LayoutNode::Transition(_) => palette.transition_dim,
        })
    }

    fn paint(grid: &Grid, palette: &Palette, colour: impl Fn(LayoutNode) -> Rgb) -> Thumbnail {
        let (columns, rows) = (grid.columns(), grid.rows());
        let mut colours = vec![palette.page; columns * rows];
        for cell in grid.cells() {
            colours[cell.row * columns + cell.column] = colour(cell.node);
        }
        Thumbnail {
            columns,
            rows,
            colours,
        }
    }

    /// How many columns wide it is: the grid's.
    pub fn columns(&self) -> usize {
        self.columns
    }

    /// How many rows high it is: the grid's.
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// The colour at a column and row, or `None` outside the grid.
    pub fn colour_at(&self, column: usize, row: usize) -> Option<Rgb> {
        if column >= self.columns || row >= self.rows {
            return None;
        }
        Some(self.colours[row * self.columns + column])
    }

    /// Every position's column, row and colour, row by row from the top
    /// left, empty positions included in the page colour: one pixel each,
    /// for a texture.
    pub fn cells(&self) -> impl Iterator<Item = (usize, usize, Rgb)> + '_ {
        let columns = self.columns.max(1);
        self.colours
            .iter()
            .enumerate()
            .map(move |(i, &colour)| (i % columns, i / columns, colour))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use petritype::{Net, NetBuilder, TakeEdge};
    use petritype_layout::Layout;

    use super::*;
    use crate::settings::ActivitySettings;

    #[test]
    fn at_glow_zero_and_one_each_kind_is_its_dim_and_bright_colour_in_either_theme() {
        for palette in [Palette::LIGHT, Palette::DARK] {
            assert_eq!(palette.place(0.0, 0.0, HueMode::Kind), palette.place_dim);
            assert_eq!(palette.place(1.0, 0.0, HueMode::Kind), palette.place_bright);
            assert_eq!(palette.transition(0.0, 0.0, false), palette.transition_dim);
            assert_eq!(
                palette.transition(1.0, 0.0, false),
                palette.transition_bright
            );
            // The flow tints the bright end only.
            assert_eq!(palette.place(0.0, 1.0, HueMode::Flow), palette.place_dim);
            assert_eq!(palette.place(1.0, 1.0, HueMode::Flow), palette.warm);
            assert_eq!(palette.place(1.0, -1.0, HueMode::Flow), palette.cold);
            assert_eq!(palette.place(1.0, 0.0, HueMode::Flow), palette.neutral);
            // Failed and running.
            assert_eq!(palette.transition(0.3, 1.0, false), palette.failed);
            assert_eq!(palette.transition(0.3, 0.0, true), palette.running);
            assert_eq!(palette.transition(0.3, 1.0, true), palette.running);
        }
    }

    #[test]
    fn the_two_themes_mirror_the_viewer() {
        // The viewer's place fill and outline, fireable greens, running,
        // failed and page colours, light and dark.
        assert_eq!(Palette::LIGHT.page, Rgb(250, 250, 250));
        assert_eq!(Palette::LIGHT.place_dim, Rgb(214, 236, 255));
        assert_eq!(Palette::LIGHT.place_bright, Rgb(43, 108, 176));
        assert_eq!(Palette::LIGHT.running, Rgb(246, 224, 94));
        assert_eq!(Palette::DARK.page, Rgb(26, 27, 31));
        assert_eq!(Palette::DARK.place_dim, Rgb(30, 58, 86));
        assert_eq!(Palette::DARK.place_bright, Rgb(99, 179, 237));
        assert_eq!(Palette::DARK.transition_dim, Rgb(39, 103, 73));
        assert_eq!(Palette::DARK.failed, Rgb(197, 48, 48));
    }

    #[test]
    fn halfway_is_halfway_and_out_of_range_clamps() {
        let p = Palette::DARK;
        assert_eq!(
            mix(Rgb(0, 100, 200), Rgb(100, 200, 0), 0.5),
            Rgb(50, 150, 100)
        );
        assert_eq!(p.place(2.0, 0.0, HueMode::Kind), p.place_bright);
        assert_eq!(p.place(-1.0, 0.0, HueMode::Kind), p.place_dim);
        assert_eq!(p.place(f32::NAN, 0.0, HueMode::Kind), p.place_dim);
    }

    /// `In -> Go -> Out`, one token in `In`.
    fn net() -> Net {
        let mut b = NetBuilder::new();
        let input = b.place::<u32>("In");
        let out = b.place::<u32>("Out");
        b.seed(input, [1]);
        b.transition("Go", (TakeEdge(input),), (out,), |n: u32| (n,));
        b.build().expect("the net builds")
    }

    #[test]
    fn a_step_lights_its_cells_and_they_fade_back_toward_the_icon() {
        let mut net = net();
        let mut layout = Layout::default();
        layout.compute(&net);
        let grid = Grid::from_layout(&layout, &net.topology());
        let mut activity = Activity::from_net(&net, ActivitySettings::default());
        let p = Palette::DARK;
        let still = Thumbnail::new(&activity, &grid, HueMode::Kind, &p);
        // In holds a token: half way under the halving curve.
        assert_eq!(
            still.colour_at(0, 0),
            Some(p.place(0.5, 0.0, HueMode::Kind))
        );
        assert_eq!(still.colour_at(0, 1), Some(p.transition_dim));
        assert_eq!(still.colour_at(0, 2), Some(p.place_dim));

        net.step().expect("Go fires");
        for event in net.drain_events() {
            activity.apply(&event);
        }
        let lit = Thumbnail::new(&activity, &grid, HueMode::Kind, &p);
        assert_eq!(lit.colour_at(0, 0), Some(p.place_bright));
        assert_eq!(lit.colour_at(0, 1), Some(p.transition_bright));
        assert_eq!(lit.colour_at(0, 2), Some(p.place_bright));

        activity.tick(Duration::from_secs(600));
        let faded = Thumbnail::new(&activity, &grid, HueMode::Kind, &p);
        assert_eq!(faded.colour_at(0, 0), Some(p.place_dim), "In is empty now");
        assert_eq!(faded.colour_at(0, 1), Some(p.transition_dim));
        assert_eq!(
            faded.colour_at(0, 2),
            Some(p.place(0.5, 0.0, HueMode::Kind)),
            "Out holds the token"
        );
        assert_eq!(
            Thumbnail::icon(&grid, &p).colour_at(0, 2),
            Some(p.place_dim)
        );
    }

    #[test]
    fn cells_cover_every_position_row_by_row_with_empty_ones_in_the_page_colour() {
        // Two sources into one transition: rank 1 and 2 have one node each,
        // so the grid is two wide with empty positions.
        let mut b = NetBuilder::new();
        let left = b.place::<u32>("Left");
        let right = b.place::<u32>("Right");
        let out = b.place::<u32>("Out");
        b.transition(
            "Join",
            (TakeEdge(left), TakeEdge(right)),
            (out,),
            |a: u32, _: u32| (a,),
        );
        let net = b.build().expect("the net builds");
        let mut layout = Layout::default();
        layout.compute(&net);
        let grid = Grid::from_layout(&layout, &net.topology());
        let icon = Thumbnail::icon(&grid, &Palette::LIGHT);
        let cells: Vec<_> = icon.cells().collect();
        assert_eq!(cells.len(), 6);
        assert_eq!(
            cells.iter().map(|&(c, r, _)| (c, r)).collect::<Vec<_>>(),
            [(0, 0), (1, 0), (0, 1), (1, 1), (0, 2), (1, 2)]
        );
        let pages = cells
            .iter()
            .filter(|&&(_, _, c)| c == Palette::LIGHT.page)
            .count();
        assert_eq!(pages, 2);
        assert_eq!(icon.colour_at(2, 0), None);
    }
}
