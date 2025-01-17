use cgmath::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct BoundingBox {
    pub min: Point3<f32>,
    pub max: Point3<f32>,
}

impl BoundingBox {
    #[allow(unused)]
    pub fn new(min: Point3<f32>, max: Point3<f32>) -> Self {
        Self { min, max }
    }
    #[allow(unused)]
    pub fn distance(&self, p: Point3<f32>) -> f32 {
        self.square_distance(p).sqrt()
    }
    #[allow(unused)]
    pub fn square_distance(&self, p: Point3<f32>) -> f32 {
        let dx = (self.min.x - p.x).max(0.0).max(p.x - self.max.x);
        let dy = (self.min.y - p.y).max(0.0).max(p.y - self.max.y);
        let dz = (self.min.z - p.z).max(0.0).max(p.z - self.max.z);
        dx * dx + dy * dy + dz * dz
    }

    #[allow(unused)]
    pub fn square_distance_xz(&self, p: Point3<f32>) -> f32 {
        let dx = (self.min.x - p.x).max(0.0).max(p.x - self.max.x);
        let dz = (self.min.z - p.z).max(0.0).max(p.z - self.max.z);
        dx * dx + dz * dz
    }
}
