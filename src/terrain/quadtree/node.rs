use crate::cache::Priority;
use crate::generate::{EARTH_CIRCUMFERENCE, EARTH_RADIUS};
use cgmath::*;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

const ROOT_SIDE_LENGTH: f32 = (EARTH_CIRCUMFERENCE * 0.25) as f32;

lazy_static! {
    pub static ref OFFSETS: [Vector2<i32>; 4] =
        [Vector2::new(0, 0), Vector2::new(1, 0), Vector2::new(0, 1), Vector2::new(1, 1),];
    pub static ref CENTER_OFFSETS: [Vector2<i32>; 4] =
        [Vector2::new(-1, -1), Vector2::new(1, -1), Vector2::new(-1, 1), Vector2::new(1, 1),];
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash, Serialize, Deserialize)]
pub(crate) struct VNode(u64);

#[allow(unused)]
impl VNode {
    // The cell sizes assume each face is covered by a texture with resolution 512x512.

    pub const LEVEL_CELL_20KM: u8 = 0; //  512 x  512 x 6   =  1.5 MB
    pub const LEVEL_CELL_10KM: u8 = 1; // 1024 x 1024 x 6   =    6 MB
    pub const LEVEL_CELL_5KM: u8 = 2; // 2048 x 2048 x 6   =   24 MB
    pub const LEVEL_CELL_2KM: u8 = 3; // 4096 x 4096 x 6   =   96 MB
    pub const LEVEL_CELL_1KM: u8 = 4; // 8192 x 8192 x 6   =  384 MB
    pub const LEVEL_CELL_625M: u8 = 5;
    pub const LEVEL_CELL_305M: u8 = 6;
    pub const LEVEL_CELL_153M: u8 = 7;
    pub const LEVEL_CELL_76M: u8 = 8;
    pub const LEVEL_CELL_38M: u8 = 9;
    pub const LEVEL_CELL_19M: u8 = 10;
    pub const LEVEL_CELL_10M: u8 = 11;
    pub const LEVEL_CELL_5M: u8 = 12;
    pub const LEVEL_CELL_2M: u8 = 13;
    pub const LEVEL_CELL_1M: u8 = 14;
    pub const LEVEL_CELL_60CM: u8 = 15;
    pub const LEVEL_CELL_30CM: u8 = 16;
    pub const LEVEL_CELL_15CM: u8 = 17;
    pub const LEVEL_CELL_7CM: u8 = 18;
    pub const LEVEL_CELL_4CM: u8 = 19;
    pub const LEVEL_CELL_2CM: u8 = 20; // 2^58 cells/face
    pub const LEVEL_CELL_1CM: u8 = 21; // 2^60 cells/face
    pub const LEVEL_CELL_5MM: u8 = 22; // 2^62 cells/face
}

impl VNode {
    fn new(level: u8, face: u8, x: u32, y: u32) -> Self {
        debug_assert!(face < 6);
        debug_assert!(level <= VNode::LEVEL_CELL_5MM);
        debug_assert!(x <= 0x3ffffff && x < (1 << level));
        debug_assert!(y <= 0x3ffffff && y < (1 << level));
        Self((level as u64) << 56 | (face as u64) << 53 | (y as u64) << 26 | (x as u64))
    }
    pub fn roots() -> [Self; 6] {
        [
            Self::new(0, 0, 0, 0),
            Self::new(0, 1, 0, 0),
            Self::new(0, 2, 0, 0),
            Self::new(0, 3, 0, 0),
            Self::new(0, 4, 0, 0),
            Self::new(0, 5, 0, 0),
        ]
    }

    pub fn x(&self) -> u32 {
        self.0 as u32 & 0x3ffffff
    }
    pub fn y(&self) -> u32 {
        (self.0 >> 26) as u32 & 0x3ffffff
    }
    pub fn level(&self) -> u8 {
        (self.0 >> 56) as u8
    }
    pub fn face(&self) -> u8 {
        (self.0 >> 53) as u8 & 0x7
    }

    pub fn aprox_side_length(&self) -> f32 {
        ROOT_SIDE_LENGTH / (1u32 << self.level()) as f32
    }

    /// Minimum distance from the center of this node on the face of a cube with coordinates from
    /// [-1, 1].
    pub fn min_distance(&self) -> f64 {
        ROOT_SIDE_LENGTH as f64 * 2.0 / (1u32 << self.level()) as f64
    }

    fn fspace_to_cspace(&self, x: f64, y: f64) -> Vector3<f64> {
        let x = x.signum() * (1.4511 - (1.4511 * 1.4511 - 1.8044 * x.abs()).sqrt()) / 0.9022;
        let y = y.signum() * (1.4511 - (1.4511 * 1.4511 - 1.8044 * y.abs()).sqrt()) / 0.9022;

        match self.face() {
            0 => Vector3::new(1.0, x, -y),
            1 => Vector3::new(-1.0, -x, -y),
            2 => Vector3::new(x, 1.0, y),
            3 => Vector3::new(-x, -1.0, y),
            4 => Vector3::new(x, -y, 1.0),
            5 => Vector3::new(-x, -y, -1.0),
            _ => unreachable!(),
        }
    }

    /// Interpolate position on this node assuming a grid with given `resolution` and surrounded by
    /// `skirt` cells outside the borders on each edge (but counted in resolution). Assumes [grid
    /// registration](https://www.ngdc.noaa.gov/mgg/global/gridregistration.html). Used for
    /// elevation data.
    ///```text
    ///       |       |
    ///     +---+---+---+
    ///   --|   |   |   |--
    ///     +---+---+---+
    ///     |   |   |   |
    ///     +---+---+---+
    ///   --|   |   |   |--
    ///     +---+---+---+
    ///       |       |
    ///```
    pub fn grid_position_cspace(
        &self,
        x: i32,
        y: i32,
        skirt: u16,
        resolution: u16,
    ) -> Vector3<f64> {
        let fx = (x - skirt as i32) as f64 / (resolution - 1 - 2 * skirt) as f64;
        let fy = (y - skirt as i32) as f64 / (resolution - 1 - 2 * skirt) as f64;
        let scale = 2.0 / (1u32 << self.level()) as f64;

        let fx = (self.x() as f64 + fx) * scale - 1.0;
        let fy = (self.y() as f64 + fy) * scale - 1.0;
        self.fspace_to_cspace(fx, fy)
    }

    /// Same as `position_cspace_corners` but uses "cell registration". Used for textures/normalmaps.
    ///```text
    ///     |       |
    ///   --+---+---+--
    ///     |   |   |
    ///     +---+---+
    ///     |   |   |
    ///   --+---+---+--
    ///     |       |
    ///```
    pub fn cell_position_cspace(
        &self,
        x: i32,
        y: i32,
        skirt: u16,
        resolution: u16,
    ) -> Vector3<f64> {
        let fx = ((x - skirt as i32) as f64 + 0.5) / (resolution - 2 * skirt) as f64;
        let fy = ((y - skirt as i32) as f64 + 0.5) / (resolution - 2 * skirt) as f64;
        let scale = 2.0 / (1u32 << self.level()) as f64;

        let fx = (self.x() as f64 + fx) * scale - 1.0;
        let fy = (self.y() as f64 + fy) * scale - 1.0;
        self.fspace_to_cspace(fx, fy)
    }

    fn cspace_to_fspace(cspace: Vector3<f64>) -> (u8, f64, f64) {
        let (face, x, y) = match (cspace.x, cspace.y, cspace.z) {
            (unit, a, b) if unit == 1.0 => (0, a, -b),
            (unit, a, b) if unit == -1.0 => (1, -a, -b),
            (a, unit, b) if unit == 1.0 => (2, a, b),
            (a, unit, b) if unit == -1.0 => (3, -a, b),
            (a, b, unit) if unit == 1.0 => (4, a, -b),
            (a, b, unit) if unit == -1.0 => (5, -a, -b),
            _ => panic!("Coordinate is not on unit cube surface"),
        };

        let x = x * (1.4511 + (1.0 - 1.4511) * x.abs());
        let y = y * (1.4511 + (1.0 - 1.4511) * y.abs());

        (face, x, y)
    }

    pub fn from_cspace(cspace: Vector3<f64>, level: u8) -> (Self, f32, f32) {
        let (face, x, y) = Self::cspace_to_fspace(cspace);

        let x = (x * 0.5 + 0.5) * (1u32 << level) as f64;
        let y = (y * 0.5 + 0.5) * (1u32 << level) as f64;

        let node = VNode::new(level, face, x.floor() as u32, y.floor() as u32);
        (node, x.fract() as f32, y.fract() as f32)
    }

    pub fn center_wspace(&self) -> Vector3<f64> {
        self.cell_position_cspace(0, 0, 0, 1).normalize() * crate::coordinates::PLANET_RADIUS
    }

    fn distance2(&self, point: Vector3<f64>) -> f64 {
        let corners = [
            self.grid_position_cspace(0, 0, 0, 2),
            self.grid_position_cspace(1, 0, 0, 2),
            self.grid_position_cspace(1, 1, 0, 2),
            self.grid_position_cspace(0, 1, 0, 2),
        ];

        let normals = [
            corners[0].cross(-corners[1]),
            corners[1].cross(-corners[2]),
            corners[2].cross(-corners[3]),
            corners[3].cross(-corners[0]),
        ];

        const MIN_RADIUS: f64 = EARTH_RADIUS - 1000.0;
        const MAX_RADIUS: f64 = EARTH_RADIUS + 9000.0;

        // Top and bottom
        if normals.iter().all(|n| n.dot(point) >= 0.0) {
            let length2 = point.dot(point);
            if length2 > MIN_RADIUS * MIN_RADIUS && length2 < MAX_RADIUS * MAX_RADIUS {
                return 0.0;
            }
            let length = length2.sqrt();
            let d = (length - MAX_RADIUS).max(MIN_RADIUS - length);
            return d * d;
        }

        // Edges
        let mut d2 = f64::INFINITY;
        for i in 0..4 {
            let corner = corners[i].normalize();
            let segment_point = point.dot(corner).min(MAX_RADIUS).max(MIN_RADIUS) * corner;
            d2 = d2.min(segment_point.distance2(point));
        }

        // Faces
        for i in 0..4 {
            if normals[i].dot(point) < 0.0
                && corners[i].cross(normals[i]).dot(point) > 0.0
                && (-corners[(i + 1) % 4]).cross(normals[i]).dot(point - corners[(i + 1) % 4]) > 0.0
            {
                let mut surface_point =
                    point - normals[i] * normals[i].dot(point) / normals[i].dot(normals[i]);
                let length2 = surface_point.dot(surface_point);
                if length2 > MAX_RADIUS * MAX_RADIUS {
                    surface_point = surface_point.normalize() * MAX_RADIUS;
                    d2 = d2.min(surface_point.distance2(point));
                } else if length2 < MIN_RADIUS * MIN_RADIUS {
                    surface_point = surface_point.normalize() * MIN_RADIUS;
                    d2 = d2.min(surface_point.distance2(point));
                } else {
                    let dot = normals[i].dot(point);
                    let length2 = dot * dot / normals[i].dot(normals[i]);
                    d2 = d2.min(length2);
                }
            }
        }

        d2
    }

    /// How much this node is needed for the current frame. Nodes with priority less than 1.0 will
    /// not be rendered (they are too detailed).
    pub(super) fn priority(&self, camera: Vector3<f64>) -> Priority {
        let min_distance = self.min_distance();
        let distance2 = self.distance2(camera);

        Priority::from_f32(((min_distance * min_distance) / distance2.max(1e-12)) as f32)
    }

    pub fn parent(&self) -> Option<(VNode, u8)> {
        if self.level() == 0 {
            return None;
        }
        let child_index = ((self.x() % 2) + (self.y() % 2) * 2) as u8;
        Some((VNode::new(self.level() - 1, self.face(), self.x() / 2, self.y() / 2), child_index))
    }

    pub fn children(&self) -> [VNode; 4] {
        assert!(self.level() < 31);
        [
            VNode::new(self.level() + 1, self.face(), self.x() * 2, self.y() * 2),
            VNode::new(self.level() + 1, self.face(), self.x() * 2 + 1, self.y() * 2),
            VNode::new(self.level() + 1, self.face(), self.x() * 2, self.y() * 2 + 1),
            VNode::new(self.level() + 1, self.face(), self.x() * 2 + 1, self.y() * 2 + 1),
        ]
    }

    pub fn find_ancestor<Visit>(&self, mut visit: Visit) -> Option<(VNode, usize, Vector2<u32>)>
    where
        Visit: FnMut(VNode) -> bool,
    {
        let mut node = *self;
        let mut generations = 0;
        let mut offset = Vector2::new(0, 0);
        while !visit(node) {
            if node.level() == 0 {
                return None;
            }
            offset += Vector2::new(node.x() & 1, node.y() & 1) * (1 << generations);
            generations += 1;
            node = VNode::new(node.level() - 1, node.face(), node.x() / 2, node.y() / 2);
        }
        Some((node, generations, offset))
    }

    pub fn breadth_first<Visit>(mut visit: Visit)
    where
        Visit: FnMut(VNode) -> bool,
    {
        let mut pending = VecDeque::new();
        for &n in &Self::roots() {
            if visit(n) {
                pending.push_back(n);
            }
        }
        while let Some(node) = pending.pop_front() {
            for &child in node.children().iter() {
                if visit(child) {
                    pending.push_back(child);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_distance() {
        let node = VNode::new(1, 1, 0, 0);
        let camera = Vector3::new(1., 0., 1.);

        let p = node.priority(camera);
        assert!(p > Priority::cutoff());
    }
}
