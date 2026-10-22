//! [`Thumbnail::svg`]: the thumbnail as SVG text, one square per cell.

use std::fmt::Write as _;

use crate::thumbnail::Thumbnail;

impl Thumbnail {
    /// The thumbnail as an SVG document, each cell a `cell_px` square: one
    /// `<rect>` per position of the grid, empty ones in the page colour,
    /// drawn with crisp edges, and nothing else. The vector form of the live
    /// picture, and of the icon ([`Thumbnail::icon`]).
    ///
    /// ```
    /// use petritype::NetBuilder;
    /// use petritype_activity::{Grid, Palette, Thumbnail};
    /// use petritype_layout::Layout;
    ///
    /// let mut b = NetBuilder::new();
    /// let _lone = b.place::<u32>("Lone");
    /// let net = b.build()?;
    /// let mut layout = Layout::default();
    /// layout.compute(&net);
    /// let grid = Grid::from_layout(&layout, &net.topology());
    ///
    /// let svg = Thumbnail::icon(&grid, &Palette::DARK).svg(8);
    /// assert_eq!(
    ///     svg,
    ///     "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"8\" height=\"8\" \
    ///      viewBox=\"0 0 8 8\" shape-rendering=\"crispEdges\">\n\
    ///      <rect x=\"0\" y=\"0\" width=\"8\" height=\"8\" fill=\"#1e3a56\"/>\n\
    ///      </svg>\n"
    /// );
    /// # Ok::<(), petritype::BuildError>(())
    /// ```
    pub fn svg(&self, cell_px: u32) -> String {
        let px = cell_px as usize;
        let (width, height) = (self.columns() * px, self.rows() * px);
        let mut out = String::new();
        // Writing to a `String` cannot fail.
        let _ = writeln!(
            out,
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" \
             viewBox=\"0 0 {width} {height}\" shape-rendering=\"crispEdges\">"
        );
        for (column, row, colour) in self.cells() {
            let _ = writeln!(
                out,
                "<rect x=\"{}\" y=\"{}\" width=\"{px}\" height=\"{px}\" fill=\"{colour}\"/>",
                column * px,
                row * px,
            );
        }
        out.push_str("</svg>\n");
        out
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use petritype_layout::Layout;

    use crate::traffic_light::build_traffic_light;
    use crate::{Activity, ActivitySettings, Grid, HueMode, Palette, Thumbnail};

    /// The traffic light, one step in (the light changed), and a car put
    /// in `Waiting` 100 ms later, drawn 300 ms after the step in the dark
    /// theme at four pixels a cell.
    #[test]
    fn the_traffic_light_draws_to_the_golden_svg() {
        let mut light = build_traffic_light().expect("the traffic light builds");
        let mut layout = Layout::default();
        layout.compute(&light.net);
        let grid = Grid::from_layout(&layout, &light.net.topology());
        let mut activity = Activity::from_net(&light.net, ActivitySettings::default());

        light.net.step().expect("Change fires");
        let events: Vec<_> = light.net.drain_events().collect();
        events.iter().for_each(|e| activity.apply(e));
        activity.tick(Duration::from_millis(100));
        light.net.put(light.waiting, "Taxi".to_owned());
        let events: Vec<_> = light.net.drain_events().collect();
        events.iter().for_each(|e| activity.apply(e));
        activity.tick(Duration::from_millis(200));

        let svg = Thumbnail::new(&activity, &grid, HueMode::Kind, &Palette::DARK).svg(4);
        assert_eq!(svg, GOLDEN, "\n{svg}");
    }

    /// `Waiting` and `Light` on the first row, `Go` and `Change` on the
    /// second, `Passed` and an empty position on the third. `Waiting` is
    /// brightest, its car 200 ms old; `Light`, touched 300 ms ago and
    /// holding one token, and `Change` are at half; `Go` and the empty
    /// `Passed` are at rest.
    const GOLDEN: &str = concat!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"8\" height=\"12\" ",
        "viewBox=\"0 0 8 12\" shape-rendering=\"crispEdges\">\n",
        "<rect x=\"0\" y=\"0\" width=\"4\" height=\"4\" fill=\"#4986b5\"/>\n",
        "<rect x=\"4\" y=\"0\" width=\"4\" height=\"4\" fill=\"#4177a2\"/>\n",
        "<rect x=\"0\" y=\"4\" width=\"4\" height=\"4\" fill=\"#276749\"/>\n",
        "<rect x=\"4\" y=\"4\" width=\"4\" height=\"4\" fill=\"#61a77f\"/>\n",
        "<rect x=\"0\" y=\"8\" width=\"4\" height=\"4\" fill=\"#1e3a56\"/>\n",
        "<rect x=\"4\" y=\"8\" width=\"4\" height=\"4\" fill=\"#1a1b1f\"/>\n",
        "</svg>\n",
    );
}
