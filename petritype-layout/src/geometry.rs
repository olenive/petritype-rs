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

    /// The straight-line distance to `other`, by [`f32::hypot`], which
    /// does not overflow on the way: two points `3e20` and `4e20` apart
    /// across and up are `5e20` apart, though either squared is past what
    /// an `f32` holds.
    pub fn distance(self, other: Self) -> f32 {
        let d = self - other;
        d.x.hypot(d.y)
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

/// A box with sides along the axes: the frame of a group, or what a whole
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
    /// along an axis it shrinks past nothing it is the box's centre on that
    /// axis: a line or a point at the middle of the box, inside it and
    /// never inside out (since 2026-11-28; until then it was a line or a
    /// point at the old `max` side moved by `by`, outside the box). A `by`
    /// that is not a number shrinks the box to its centre the same way.
    ///
    /// ```
    /// use petritype_layout::{Bounds, Point};
    ///
    /// let wide = Bounds::from_corners(Point::new(0.0, 0.0), Point::new(10.0, 4.0));
    /// assert_eq!(
    ///     wide.inflate(1.0),
    ///     Bounds::from_corners(Point::new(-1.0, -1.0), Point::new(11.0, 5.0))
    /// );
    /// // Shrunk by 3: across it is 4 wide, and up it is the middle.
    /// assert_eq!(
    ///     wide.inflate(-3.0),
    ///     Bounds::from_corners(Point::new(3.0, 2.0), Point::new(7.0, 2.0))
    /// );
    /// assert_eq!(wide.inflate(-100.0), Bounds::from_corners(wide.center(), wide.center()));
    /// ```
    pub fn inflate(&self, by: f32) -> Self {
        let centre = self.center();
        // One axis: its ends moved out by `by`, or both at the centre when
        // they would cross.
        let axis = |min: f32, max: f32, centre: f32| {
            let (low, high) = (min - by, max + by);
            if low <= high {
                (low, high)
            } else {
                (centre, centre)
            }
        };
        let (min_x, max_x) = axis(self.min.x, self.max.x, centre.x);
        let (min_y, max_y) = axis(self.min.y, self.max.y, centre.y);
        Self {
            min: Point::new(min_x, min_y),
            max: Point::new(max_x, max_y),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_distance_too_large_to_square_is_still_measured() {
        let far = Point::new(3e20, 4e20);
        assert_eq!(Point::ZERO.distance(far), 5e20);
        assert_eq!(Point::new(1.0, 1.0).distance(Point::new(4.0, 5.0)), 5.0);
    }

    #[test]
    fn a_box_shrunk_past_nothing_is_its_centre_and_stays_inside_it() {
        let tall = Bounds::from_corners(Point::new(-2.0, 0.0), Point::new(2.0, 30.0));
        let shrunk = tall.inflate(-5.0);
        assert_eq!(
            shrunk,
            Bounds::from_corners(Point::new(0.0, 5.0), Point::new(0.0, 25.0)),
            "a line down the middle"
        );
        assert!(tall.contains(shrunk.min) && tall.contains(shrunk.max));
        let gone = tall.inflate(-100.0);
        assert_eq!((gone.min, gone.max), (tall.center(), tall.center()));
        assert!(gone.min.x <= gone.max.x && gone.min.y <= gone.max.y);
        // Exactly to nothing is a line or a point where the sides meet.
        assert_eq!(tall.inflate(-2.0).size(), Point::new(0.0, 26.0));
        assert_eq!(
            tall.inflate(f32::NAN),
            Bounds::from_corners(tall.center(), tall.center())
        );
        assert_eq!(tall.inflate(0.0), tall);
    }
}
