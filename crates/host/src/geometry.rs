//! Physical-pixel geometry shared by the monitor policy and native adapters.
//!
//! All values are physical pixels. Logical sizing is converted before a
//! [`Rect`] is handed to a native placement call, so mixed-DPI monitors never
//! mix units in one calculation.

/// A physical-pixel screen coordinate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Point {
    /// Horizontal position.
    pub x: i32,
    /// Vertical position.
    pub y: i32,
}

impl Point {
    /// Creates a point from physical pixel coordinates.
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

/// A physical-pixel size.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Size {
    /// Width in physical pixels.
    pub width: i32,
    /// Height in physical pixels.
    pub height: i32,
}

impl Size {
    /// Creates a size from physical pixels.
    pub const fn new(width: i32, height: i32) -> Self {
        Self { width, height }
    }

    /// Converts a logical size at `scale_factor` into physical pixels.
    ///
    /// A missing, zero, negative or non-finite scale is treated as 1.0 rather
    /// than producing a nonsense rectangle. The result is rounded to the
    /// nearest physical pixel, which is what Win32 placement expects.
    pub fn scaled(self, scale_factor: f64) -> Self {
        let scale = if scale_factor.is_finite() && scale_factor > 0.0 {
            scale_factor
        } else {
            1.0
        };
        Self {
            width: (f64::from(self.width.max(0)) * scale).round() as i32,
            height: (f64::from(self.height.max(0)) * scale).round() as i32,
        }
    }
}

/// A physical-pixel rectangle. `x`/`y` are the top-left origin and `width`/
/// `height` are non-negative by convention for placement results.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Rect {
    /// Left edge in physical pixels.
    pub x: i32,
    /// Top edge in physical pixels.
    pub y: i32,
    /// Width in physical pixels.
    pub width: i32,
    /// Height in physical pixels.
    pub height: i32,
}

impl Rect {
    /// Creates a rectangle from a physical-pixel origin and size.
    pub const fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Exclusive right edge, saturating at the integer limits.
    pub const fn right(&self) -> i32 {
        self.x.saturating_add(self.width)
    }

    /// Exclusive bottom edge, saturating at the integer limits.
    pub const fn bottom(&self) -> i32 {
        self.y.saturating_add(self.height)
    }

    /// Returns `true` when the rectangle has no positive area.
    pub const fn is_empty(&self) -> bool {
        self.width <= 0 || self.height <= 0
    }

    /// Returns the integer centre of the rectangle.
    pub fn center(&self) -> Point {
        Point {
            x: self.x.saturating_add(self.width / 2),
            y: self.y.saturating_add(self.height / 2),
        }
    }

    /// Returns `true` when `point` is inside the half-open rectangle.
    pub fn contains_point(&self, point: Point) -> bool {
        !self.is_empty()
            && point.x >= self.x
            && point.y >= self.y
            && point.x < self.right()
            && point.y < self.bottom()
    }

    /// Returns `true` when `other` lies entirely inside this rectangle.
    pub fn contains_rect(&self, other: Rect) -> bool {
        !self.is_empty()
            && !other.is_empty()
            && other.x >= self.x
            && other.y >= self.y
            && other.right() <= self.right()
            && other.bottom() <= self.bottom()
    }

    /// Returns `true` when the two rectangles share a positive area.
    pub fn intersects(&self, other: Rect) -> bool {
        !self.is_empty()
            && !other.is_empty()
            && self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }

    /// Returns the shared area in physical square pixels, or zero.
    pub fn intersection_area(&self, other: Rect) -> i64 {
        if !self.intersects(other) {
            return 0;
        }
        let width = i64::from(self.right().min(other.right())) - i64::from(self.x.max(other.x));
        let height = i64::from(self.bottom().min(other.bottom())) - i64::from(self.y.max(other.y));
        width.max(0) * height.max(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaling_converts_logical_to_physical_at_common_windows_factors() {
        let logical = Size::new(760, 500);
        assert_eq!(logical.scaled(1.0), Size::new(760, 500));
        assert_eq!(logical.scaled(1.5), Size::new(1140, 750));
        assert_eq!(logical.scaled(2.0), Size::new(1520, 1000));
    }

    #[test]
    fn scaling_rejects_nonsense_factors_instead_of_guessing() {
        let logical = Size::new(100, 40);
        for factor in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(logical.scaled(factor), logical);
        }
    }

    #[test]
    fn half_open_containment_and_intersection_are_exact() {
        let rect = Rect::new(-10, -10, 20, 20);
        assert!(rect.contains_point(Point::new(-10, -10)));
        assert!(rect.contains_point(Point::new(9, 9)));
        assert!(!rect.contains_point(Point::new(10, 9)));
        assert!(!rect.contains_point(Point::new(9, 10)));

        assert!(rect.intersects(Rect::new(0, 0, 5, 5)));
        assert!(!rect.intersects(Rect::new(10, 10, 5, 5)));
        assert_eq!(
            rect.intersection_area(Rect::new(0, 0, 10, 10)),
            10 * 10,
            "shared area must be measured in physical pixels"
        );
        assert_eq!(rect.intersection_area(Rect::new(50, 50, 10, 10)), 0);
    }
}
