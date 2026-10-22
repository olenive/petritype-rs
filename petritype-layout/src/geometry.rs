//! The two value types the layout speaks in: a [`Point`], and the
//! [`Bounds`] around some points.

use std::ops::{Add, AddAssign, Div, Mul, Sub, SubAssign};

/// A position in the layout's world coordinates, or the offset between two
/// positions. `x` grows rightward and `y` upward.
///
/// A renderer converts it to its own vector type from
/// [`to_array`](Point::to_array) or from the fields.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    /// Across, growing rightward.
    pub x: f32,
    /// Up, growing upward.
    pub y: f32,
}

impl Point {
    /// The origin, which is also the offset of nothing.
    pub const ZERO: Self = Self::new(0.0, 0.0);

    /// The point `x` across and `y` up.
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    /// The coordinates as `[x, y]`.
    pub const fn to_array(self) -> [f32; 2] {
        [self.x, self.y]
    }

    /// The straight-line distance to `other`.
    pub fn distance(self, other: Self) -> f32 {
        let d = self - other;
        (d.x * d.x + d.y * d.y).sqrt()
    }

    /// The smaller of each coordinate.
    fn lower(self, other: Self) -> Self {
        Self::new(self.x.min(other.x), self.y.min(other.y))
    }

    /// The larger of each coordinate.
    fn upper(self, other: Self) -> Self {
        Self::new(self.x.max(other.x), self.y.max(other.y))
    }
}

impl From<[f32; 2]> for Point {
    fn from([x, y]: [f32; 2]) -> Self {
        Self::new(x, y)
    }
}

impl Add for Point {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self::new(self.x + other.x, self.y + other.y)
    }
}

impl AddAssign for Point {
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}

impl Sub for Point {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y)
    }
}

impl SubAssign for Point {
    fn sub_assign(&mut self, other: Self) {
        *self = *self - other;
    }
}

/// Scales both coordinates.
impl Mul<f32> for Point {
    type Output = Self;

    fn mul(self, factor: f32) -> Self {
        Self::new(self.x * factor, self.y * factor)
    }
}

/// Divides both coordinates.
impl Div<f32> for Point {
    type Output = Self;

    fn div(self, divisor: f32) -> Self {
        Self::new(self.x / divisor, self.y / divisor)
    }
}

/// A box with sides along the axes: the frame of a cluster, or what a whole
/// drawing covers.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Bounds {
    /// The corner with the smaller coordinates: bottom left.
    pub min: Point,
    /// The corner with the larger coordinates: top right.
    pub max: Point,
}

impl Bounds {
    /// The box with corners `a` and `b`, given in either order.
    pub fn from_corners(a: Point, b: Point) -> Self {
        Self {
            min: a.lower(b),
            max: a.upper(b),
        }
    }

    /// The box reaching `half` to either side of `center`.
    pub fn from_center_half_size(center: Point, half: Point) -> Self {
        Self {
            min: center - half,
            max: center + half,
        }
    }

    /// The middle of the box.
    pub fn center(&self) -> Point {
        (self.min + self.max) * 0.5
    }

    /// The width and height of the box.
    pub fn size(&self) -> Point {
        self.max - self.min
    }

    /// The smallest box holding both this one and `other`.
    pub fn union(&self, other: Self) -> Self {
        Self {
            min: self.min.lower(other.min),
            max: self.max.upper(other.max),
        }
    }

    /// The smallest box holding both this one and `point`.
    pub fn union_point(&self, point: Point) -> Self {
        Self {
            min: self.min.lower(point),
            max: self.max.upper(point),
        }
    }

    /// The box grown by `by` on every side. A negative `by` shrinks it, and
    /// a box shrunk past nothing is a line or a point at its old `max`
    /// side, never inside out.
    pub fn inflate(&self, by: f32) -> Self {
        let max = self.max + Point::new(by, by);
        Self {
            min: (self.min - Point::new(by, by)).lower(max),
            max,
        }
    }

    /// Whether `point` is inside the box or on its edge.
    pub fn contains(&self, point: Point) -> bool {
        self.min.x <= point.x
            && point.x <= self.max.x
            && self.min.y <= point.y
            && point.y <= self.max.y
    }
}
