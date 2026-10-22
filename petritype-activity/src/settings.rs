//! [`ActivitySettings`]: every value the activity model and its drawing are
//! tuned by, as plain data.

/// Which clock a layer of levels decays against.
///
/// With the `serde` feature it is written in snake case, `"wall"` or
/// `"firings"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum Clock {
    /// Seconds. The level halves every so many seconds whatever the net
    /// does, so a net that stops goes dark, which is how a stall shows.
    #[default]
    Wall,
    /// Firings of the net as a whole, counted at each launch or fire. The
    /// level halves every so many firings anywhere in the net, so the
    /// picture depends on the sequence of events and not on its pace: a
    /// replay looks the same at any speed. A net that stalls keeps its glow,
    /// which the staleness dim
    /// ([`stale_half_life_secs`](ActivitySettings::stale_half_life_secs))
    /// answers by fading it toward a floor.
    Firings,
}

/// One layer of levels, the pulse or the ember: how far a hit raises it and
/// how fast it decays.
///
/// Both half-lives are kept, one per [`Clock`], so that switching the clock
/// back and forth keeps each value as it was tuned.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct LayerSettings {
    /// How much one hit adds to the level, which then clamps at 1.
    pub jump: f32,
    /// The clock the level decays against.
    pub clock: Clock,
    /// The half-life in seconds, used under [`Clock::Wall`].
    pub half_life_secs: f32,
    /// The half-life in firings, used under [`Clock::Firings`].
    pub half_life_firings: f32,
}

impl LayerSettings {
    /// The pulse's starting values: a jump of 1, and a half-life of 300 ms
    /// on the wall clock or 3 firings on the firings clock.
    pub const PULSE: LayerSettings = LayerSettings {
        jump: 1.0,
        clock: Clock::Wall,
        half_life_secs: 0.3,
        half_life_firings: 3.0,
    };

    /// The ember's starting values: a jump of a twentieth, and a half-life
    /// of 20 s on the wall clock or 50 firings on the firings clock.
    pub const EMBER: LayerSettings = LayerSettings {
        jump: 0.05,
        clock: Clock::Wall,
        half_life_secs: 20.0,
        half_life_firings: 50.0,
    };

    /// What a level of this layer is multiplied by when `clock` advances by
    /// `amount` (seconds or firings): `2^(-amount / half_life)` under this
    /// layer's clock, and 1 under the other. A half-life of zero or less
    /// takes the level straight to 0.
    pub fn decay(&self, clock: Clock, amount: f32) -> f32 {
        if clock != self.clock {
            return 1.0;
        }
        let half_life = match clock {
            Clock::Wall => self.half_life_secs,
            Clock::Firings => self.half_life_firings,
        };
        halving(half_life, amount)
    }
}

/// `2^(-amount / half_life)`, with nothing elapsed giving 1 and a half-life
/// of zero or less giving 0.
pub(crate) fn halving(half_life: f32, amount: f32) -> f32 {
    if amount <= 0.0 || amount.is_nan() {
        1.0
    } else if half_life <= 0.0 || half_life.is_nan() {
        0.0
    } else {
        (-amount / half_life).exp2()
    }
}

/// How a node's two levels make its glow. The settings window's toggle
/// *Glow adds up* is off for [`Peak`](GlowMode::Peak) and on for
/// [`Sum`](GlowMode::Sum).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum GlowMode {
    /// The larger of the pulse and the ember. Every pulse keeps its shape
    /// against the ember, and no node is ever brighter than one hit.
    #[default]
    Peak,
    /// The pulse and the ember added, clamped at 1. A node both recently and
    /// persistently active reads brighter than one that is merely recent,
    /// and pulses on a busy node blur together.
    Sum,
}

impl GlowMode {
    /// The glow from a pulse and an ember, in 0 to 1.
    pub fn combine(self, pulse: f32, ember: f32) -> f32 {
        match self {
            GlowMode::Peak => pulse.max(ember),
            GlowMode::Sum => (pulse + ember).min(1.0),
        }
        .clamp(0.0, 1.0)
    }
}

/// How a place's standing token count makes its base glow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum CountCurve {
    /// `1 - 2^(-count / half_count)`, with
    /// [`half_count`](ActivitySettings::half_count). With a half-count of 1,
    /// one token gives a half, two three quarters and three seven eighths.
    /// It saturates quickly, so a queue of five and one of fifty look
    /// alike; a larger half-count stretches it.
    #[default]
    Halving,
    /// `log2(1 + count) / log2(1 + cap)`, clamped to 1, with
    /// [`count_cap`](ActivitySettings::count_cap). One token is dim, and
    /// growth stays visible up to the cap: the curve for watching a backlog
    /// build.
    Log,
}

/// What a thumbnail's cells are tinted by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum HueMode {
    /// Places one hue and transitions another, from the theme.
    #[default]
    Kind,
    /// A place's hue runs cold through neutral to warm by its flow, blue
    /// when it drains and orange when it fills; transitions keep their
    /// green.
    Flow,
}

/// Every value the activity model and its drawing are tuned by: a plain
/// struct, so that a file per net, a command line, a settings window and the
/// tests all set the same values.
///
/// The defaults are the starting values, to be tuned by looking.
///
/// With the `serde` feature it is `Serialize` and `Deserialize`, and a field
/// left out of the input takes its default, so a file need only name what it
/// changes: `{"glow": "sum"}` is the defaults with the glow adding up. A
/// layer that is given is given whole.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct ActivitySettings {
    /// The fast layer, which says what is happening now.
    pub pulse: LayerSettings,
    /// The slow layer, which says what the recent pattern is. Flow moves on
    /// its clock and half-life too.
    ///
    /// A place takes one ember hit per event that touches it, however many
    /// tokens move, so the ember counts events and the pulse counts tokens.
    /// Under a steady run of hits the ember settles where its decay between
    /// hits matches the jump, which is how to tune it: on the wall clock
    /// near `jump × half_life × rate × 1.44`, with `rate` the node's hits a
    /// second, and on the firings clock near
    /// `jump × half_life × share × 1.44`, with `share` the fraction of the
    /// net's firings that hit the node (1.44 is `1 / ln 2`). At the
    /// defaults, a jump of 0.05 and a half-life of 20 s, a node saturates
    /// above 0.7 hits a second; with a half-life of 50 firings, above a 28%
    /// share, and an evenly busy node of a seven-node net, hit by a seventh
    /// of the firings, sits near a half.
    pub ember: LayerSettings,
    /// The hit a transition takes when it distributes, as a fraction of each
    /// layer's jump: 0.5 by default.
    pub distribute_hit: f32,
    /// The staleness dim's half-life in seconds: while some layer runs on
    /// the [firings clock](Clock::Firings), the whole thumbnail fades toward
    /// its [floor](ActivitySettings::stale_floor) at this rate, and the next
    /// fire, launch, distribute or failure restores it. `None` turns the dim
    /// off. 30 s by default.
    pub stale_half_life_secs: Option<f32>,
    /// The fraction of its glow the staleness dim leaves however long the
    /// stall, 0 to 1: a quarter by default, so a stalled net reads as
    /// stalled and its pattern is still there.
    pub stale_floor: f32,
    /// How the two levels make a glow.
    pub glow: GlowMode,
    /// How a place's token count makes its base glow.
    pub count_curve: CountCurve,
    /// The count at which [`CountCurve::Halving`] gives half brightness: 1
    /// by default.
    pub half_count: f32,
    /// The count at which [`CountCurve::Log`] reaches full brightness: 16 by
    /// default.
    pub count_cap: f32,
    /// How far each token added raises a place's flow, and each token taken
    /// lowers it: 0.25 by default.
    pub flow_gain: f32,
    /// What the thumbnail's cells are tinted by.
    pub hue: HueMode,
    /// Whether the main drawing takes its colours from the decaying levels
    /// under Activate, rather than the viewer's fixed highlight: on by
    /// default. The model does not read it; a viewer does.
    pub decay_in_drawing: bool,
}

impl Default for ActivitySettings {
    fn default() -> Self {
        Self {
            pulse: LayerSettings::PULSE,
            ember: LayerSettings::EMBER,
            distribute_hit: 0.5,
            stale_half_life_secs: Some(30.0),
            stale_floor: 0.25,
            glow: GlowMode::Peak,
            count_curve: CountCurve::Halving,
            half_count: 1.0,
            count_cap: 16.0,
            flow_gain: 0.25,
            hue: HueMode::Kind,
            decay_in_drawing: true,
        }
    }
}

impl ActivitySettings {
    /// The base glow of a place holding `count` tokens, by the
    /// [count curve](ActivitySettings::count_curve), in 0 to 1. No tokens
    /// give 0 under either curve.
    pub fn base(&self, count: usize) -> f32 {
        if count == 0 {
            return 0.0;
        }
        let count = count as f32;
        match self.count_curve {
            CountCurve::Halving => 1.0 - halving(self.half_count, count),
            CountCurve::Log if self.count_cap > 0.0 => {
                ((1.0 + count).log2() / (1.0 + self.count_cap).log2()).min(1.0)
            }
            CountCurve::Log => 1.0,
        }
    }

    /// Whether the staleness dim is in force: it is on, and some layer runs
    /// on the firings clock. Under the wall clock alone a stall goes dark by
    /// itself.
    pub fn stale_dim(&self) -> bool {
        self.stale_half_life_secs.is_some()
            && (self.pulse.clock == Clock::Firings || self.ember.clock == Clock::Firings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn the_defaults_are_the_starting_values() {
        let s = ActivitySettings::default();
        assert_eq!((s.pulse.jump, s.pulse.half_life_secs), (1.0, 0.3));
        assert_eq!(s.pulse.half_life_firings, 3.0);
        assert_eq!((s.ember.jump, s.ember.half_life_secs), (0.05, 20.0));
        assert_eq!(s.ember.half_life_firings, 50.0);
        assert_eq!((s.pulse.clock, s.ember.clock), (Clock::Wall, Clock::Wall));
        assert_eq!(s.distribute_hit, 0.5);
        assert_eq!(s.stale_half_life_secs, Some(30.0));
        assert_eq!(s.stale_floor, 0.25);
        assert_eq!(s.glow, GlowMode::Peak);
        assert_eq!(s.count_curve, CountCurve::Halving);
        assert_eq!((s.half_count, s.count_cap), (1.0, 16.0));
        assert_eq!(s.flow_gain, 0.25);
        assert_eq!(s.hue, HueMode::Kind);
        assert!(s.decay_in_drawing);
        assert!(!s.stale_dim(), "both layers start on the wall clock");
    }

    #[test]
    fn the_halving_curve_gives_a_half_three_quarters_and_seven_eighths() {
        let s = ActivitySettings::default();
        assert_eq!(s.base(0), 0.0);
        assert!(close(s.base(1), 0.5));
        assert!(close(s.base(2), 0.75));
        assert!(close(s.base(3), 0.875));
        // A larger half-count stretches it: one token is dimmer.
        let stretched = ActivitySettings {
            half_count: 4.0,
            ..ActivitySettings::default()
        };
        assert!(close(stretched.base(4), 0.5));
        assert!(stretched.base(1) < s.base(1));
    }

    #[test]
    fn the_log_curve_reaches_one_at_its_cap() {
        let s = ActivitySettings {
            count_curve: CountCurve::Log,
            ..ActivitySettings::default()
        };
        assert_eq!(s.base(0), 0.0);
        assert!(close(s.base(16), 1.0));
        assert_eq!(s.base(100), 1.0, "clamped past the cap");
        // log2(2) / log2(17): one token is dim.
        assert!(close(s.base(1), 1.0 / 17f32.log2()));
        assert!(s.base(8) < s.base(15), "growth stays visible below the cap");
    }

    #[test]
    fn peak_takes_the_larger_and_sum_adds_and_clamps() {
        assert_eq!(GlowMode::Peak.combine(0.3, 0.5), 0.5);
        assert_eq!(GlowMode::Peak.combine(0.9, 0.5), 0.9);
        assert!(close(GlowMode::Sum.combine(0.3, 0.5), 0.8));
        assert_eq!(GlowMode::Sum.combine(0.9, 0.5), 1.0);
        assert_eq!(GlowMode::Peak.combine(0.0, 0.0), 0.0);
    }

    #[test]
    fn a_layer_decays_on_its_own_clock_only() {
        let pulse = LayerSettings::PULSE;
        assert!(close(pulse.decay(Clock::Wall, 0.3), 0.5));
        assert!(close(pulse.decay(Clock::Wall, 0.6), 0.25));
        assert_eq!(pulse.decay(Clock::Firings, 3.0), 1.0);
        assert_eq!(pulse.decay(Clock::Wall, 0.0), 1.0);
        let instant = LayerSettings {
            half_life_secs: 0.0,
            ..pulse
        };
        assert_eq!(instant.decay(Clock::Wall, 0.01), 0.0);
        assert_eq!(instant.decay(Clock::Wall, 0.0), 1.0);
    }

    #[test]
    fn the_stale_dim_is_in_force_only_with_a_firings_clock() {
        let mut s = ActivitySettings::default();
        s.ember.clock = Clock::Firings;
        assert!(s.stale_dim());
        s.stale_half_life_secs = None;
        assert!(!s.stale_dim());
    }

    #[cfg(feature = "serde")]
    #[test]
    fn settings_round_trip_as_json_and_a_partial_file_fills_from_the_defaults() {
        let mut s = ActivitySettings::default();
        s.pulse.clock = Clock::Firings;
        s.glow = GlowMode::Sum;
        s.stale_half_life_secs = None;
        let text = serde_json::to_string(&s).expect("settings serialise");
        assert!(text.contains("\"clock\":\"firings\""), "{text}");
        let back: ActivitySettings = serde_json::from_str(&text).expect("and read back");
        assert_eq!(back, s);

        let partial: ActivitySettings =
            serde_json::from_str(r#"{"glow": "sum", "count_curve": "log"}"#).expect("partial");
        assert_eq!(
            partial,
            ActivitySettings {
                glow: GlowMode::Sum,
                count_curve: CountCurve::Log,
                ..ActivitySettings::default()
            }
        );
    }
}
