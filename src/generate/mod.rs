use crate::cache::{AssetLoadContext, MMappedAsset, WebAsset};
use crate::coordinates::CoordinateSystem;
use crate::mapfile::MapFile;
use crate::srgb::SRGB_TO_LINEAR;
use crate::terrain::dem::DemSource;
use crate::terrain::dem::GlobalDem;
use crate::terrain::heightmap::{self, Heightmap};
use crate::terrain::landcover::{BlueMarble, BlueMarbleTileSource};
use crate::terrain::quadtree::VNode;
use crate::terrain::raster::RasterCache;
use crate::terrain::reprojected_raster::{
    DataType, RasterSource, ReprojectedDemDef, ReprojectedRaster, ReprojectedRasterDef,
};
use crate::terrain::tile_cache::{
    LayerParams, LayerType, NoiseParams, TextureDescriptor, TextureFormat, TileHeader,
};
use byteorder::{LittleEndian, WriteBytesExt};
use failure::Error;
use maplit::hashmap;
use rand;
use rand::distributions::Distribution;
use rand_distr::Normal;
use std::cell::RefCell;
use std::f64::consts::PI;
use std::io::Write;
use std::rc::Rc;
use vec_map::VecMap;

mod gpu;
pub(crate) use gpu::*;

/// The radius of the earth in meters.
pub(crate) const EARTH_RADIUS: f64 = 6371000.0;
pub(crate) const EARTH_CIRCUMFERENCE: f64 = 2.0 * PI * EARTH_RADIUS;

// Mapping from side length to level number.
#[allow(unused)]
mod levels {
    pub const LEVEL_10000_KM: i32 = 0;
    pub const LEVEL_5000_KM: i32 = 1;
    pub const LEVEL_2500_KM: i32 = 2;
    pub const LEVEL_1250_KM: i32 = 3;
    pub const LEVEL_625_KM: i32 = 4;
    pub const LEVEL_300_KM: i32 = 5;
    pub const LEVEL_150_KM: i32 = 6;
    pub const LEVEL_75_KM: i32 = 7;
    pub const LEVEL_40_KM: i32 = 8;
    pub const LEVEL_20_KM: i32 = 9;
    pub const LEVEL_10_KM: i32 = 10;
    pub const LEVEL_5_KM: i32 = 11;
    pub const LEVEL_2_KM: i32 = 12;
    pub const LEVEL_1_KM: i32 = 13;
    pub const LEVEL_600_M: i32 = 14;
    pub const LEVEL_305_M: i32 = 15;
    pub const LEVEL_153_M: i32 = 16;
    pub const LEVEL_76_M: i32 = 17;
    pub const LEVEL_38_M: i32 = 18;
    pub const LEVEL_19_M: i32 = 19;
    pub const LEVEL_10_M: i32 = 20;
    pub const LEVEL_5_M: i32 = 21;
    pub const LEVEL_2_M: i32 = 22;
    pub const LEVEL_1_M: i32 = 23;
    pub const LEVEL_60_CM: i32 = 24;
    pub const LEVEL_30_CM: i32 = 25;
    pub const LEVEL_15_CM: i32 = 26;
}
use levels::*;

/// How much detail the terrain mesh should have. Higher values require more resources to render,
/// but produce nicer results.
pub enum VertexQuality {
    /// Use up to about 4M triangles per frame.
    High,
    /// About 1M triangles per frame.
    Medium,
    /// A couple hundred thousand triangles per frame.
    Low,
}
impl VertexQuality {
    fn resolution(&self) -> u16 {
        match *self {
            VertexQuality::Low => 33,
            VertexQuality::Medium => 65,
            VertexQuality::High => 129,
        }
    }
    fn resolution_log2(&self) -> u32 {
        let r = self.resolution() - 1;
        assert!(r.is_power_of_two());
        r.trailing_zeros()
    }
    fn as_str(&self) -> &str {
        match *self {
            VertexQuality::Low => "vl",
            VertexQuality::Medium => "vm",
            VertexQuality::High => "vh",
        }
    }
}

/// What resolution to use for terrain texture mapping. Higher values consume much more GPU memory
/// and increase the file size.
pub enum TextureQuality {
    /// Quality suitable for a 4K display.
    Ultra,
    /// Good for resolutions up to 1080p.
    High,
    /// About half the quality `High`.
    Low,
}
impl TextureQuality {
    fn resolution(&self) -> u16 {
        match *self {
            TextureQuality::Low => 256,
            TextureQuality::High => 512,
            TextureQuality::Ultra => 1024,
        }
    }
    fn as_str(&self) -> &str {
        match *self {
            TextureQuality::Low => "tl",
            TextureQuality::High => "th",
            TextureQuality::Ultra => "tu",
        }
    }
}

/// Used to construct a `QuadTree`.
pub struct MapFileBuilder {
    source: DemSource,
    vertex_quality: VertexQuality,
    texture_quality: TextureQuality,
    // materials: MaterialSet<R>,
    // sky: Skybox<R>,
    context: Option<AssetLoadContext>,
}
impl MapFileBuilder {
    /// Create a new `QuadTreeBuilder` with default arguments.
    ///
    /// At very least, the latitude and longitude should probably be set to their desired values
    /// before calling `build()`.
    pub fn new() -> Self {
        let context = AssetLoadContext::new();
        Self {
            source: DemSource::Srtm30m,
            vertex_quality: VertexQuality::High,
            texture_quality: TextureQuality::High,
            // materials: MaterialSet::load(&mut factory, encoder, &mut context).unwrap(),
            // sky: Skybox::new(&mut factory, encoder, &mut context),
            context: Some(context),
            // factory,
            // encoder,
        }
    }

    /// How detailed the resulting terrain mesh should be.
    pub fn vertex_quality(mut self, quality: VertexQuality) -> Self {
        self.vertex_quality = quality;
        self
    }

    /// How high resolution the terrain's textures should be.
    pub fn texture_quality(mut self, quality: TextureQuality) -> Self {
        self.texture_quality = quality;
        self
    }

    /// Actually construct the `QuadTree`.
    ///
    /// This function will (the first time it is called) download many gigabytes of raw data,
    /// primarily datasets relating to real world land cover and elevation. These files will be
    /// stored in ~/.terra, so that they don't have to be fetched multiple times. This means that
    /// this function can largely resume from where it left off if interrupted.
    ///
    /// Even once all needed files have been downloaded, the generation process takes a large amount
    /// of CPU resources. You can expect it to run at full load continiously for several full
    /// minutes, even in release builds (you *really* don't want to wait for generation in debug
    /// mode...).
    pub fn build(mut self) -> Result<MapFile, Error> {
        let mut context = self.context.take().unwrap();
        let (header, _) = self.load(&mut context)?;

        Ok(MapFile::new(header))
    }

    fn name(&self) -> String {
        format!(
            "{}m_{}_{}",
            self.source.resolution(),
            self.vertex_quality.as_str(),
            self.texture_quality.as_str(),
        )
    }
}

impl MMappedAsset for MapFileBuilder {
    type Header = TileHeader;

    fn filename(&self) -> String {
        format!("maps/{}", self.name())
    }

    fn generate<W: Write>(
        &self,
        context: &mut AssetLoadContext,
        mut writer: W,
    ) -> Result<Self::Header, Error> {
        writer.write_all(&[0])?;

        // Cell size in the y (latitude) direction, in meters. The x (longitude) direction will have
        // smaller cell sizes due to the projection.
        let dem_cell_size_y =
            self.source.cell_size() / (360.0 * 60.0 * 60.0) * EARTH_CIRCUMFERENCE as f32;

        let resolution_ratio =
            self.texture_quality.resolution() / (self.vertex_quality.resolution() - 1);
        assert!(resolution_ratio > 0);

        let world_size = 4194304.0;
        let max_heights_present_level = LEVEL_1_KM - self.vertex_quality.resolution_log2() as i32;
        let max_texture_present_level =
            max_heights_present_level - (resolution_ratio as f32).log2() as i32;

        let max_heights_level = max_heights_present_level;
        let max_texture_level = max_heights_present_level;

        let cell_size = world_size / ((self.vertex_quality.resolution() - 1) as f32)
            * (0.5f32).powi(max_heights_level);
        let num_fractal_levels = (dem_cell_size_y / cell_size).log2().ceil().max(0.0) as i32;
        let max_dem_level = max_texture_level - num_fractal_levels.max(0).min(max_texture_level);

        // Amount of space outside of tile that is included in heightmap. Used for computing
        // normals and such. Must be even.
        let skirt = 4;
        assert_eq!(skirt % 2, 0);

        // Resolution of each heightmap stored in heightmaps. They are at higher resolution than
        // self.vertex_quality.resolution() so that the more detailed textures can be derived from
        // them.
        let heightmap_resolution = self.texture_quality.resolution() + 1 + 2 * skirt;
        let heights_resolution = self.vertex_quality.resolution();
        let normalmap_resolution = heightmap_resolution - 5;
        let colormap_resolution = heightmap_resolution - 5;
        let colormap_skirt = skirt - 2;

        let layers: VecMap<LayerParams> = hashmap![
            LayerType::Heightmaps.index() => LayerParams {
                    layer_type: LayerType::Heightmaps,
                    texture_resolution: heightmap_resolution as u32,
                    texture_border_size: skirt as u32,
                    texture_format: TextureFormat::R32F,
                },
            LayerType::Displacements.index() => LayerParams {
                    layer_type: LayerType::Displacements,
                    texture_resolution: heights_resolution as u32,
                    texture_border_size: 0,
                    texture_format: TextureFormat::RGBA32F,
                },
            LayerType::Albedo.index() => LayerParams {
                    layer_type: LayerType::Albedo,
                    texture_resolution: colormap_resolution as u32,
                    texture_border_size: colormap_skirt as u32,
                    texture_format: TextureFormat::RGBA8,
                },
            LayerType::Normals.index() => LayerParams {
                    layer_type: LayerType::Normals,
                    texture_resolution: normalmap_resolution as u32,
                    texture_border_size: skirt as u32 - 2,
                    texture_format: TextureFormat::RG8,
                },
        ]
        .into_iter()
        .collect();
        let noise = State::generate_noise(context)?;
        let tile_header = TileHeader { layers, noise };

        let mapfile = MapFile::new(tile_header.clone());

        VNode::breadth_first(|n| {
            mapfile.set_missing(LayerType::Heightmaps, n, true).unwrap();
            n.level() < 3
        });
        VNode::breadth_first(|n| {
            mapfile.set_missing(LayerType::Albedo, n, true).unwrap();
            n.level() < 3
        });

        let mut state = State {
            random: {
                let normal = Normal::new(0.0, 1.0).unwrap();
                let v =
                    (0..(15 * 15)).map(|_| normal.sample(&mut rand::thread_rng()) as f32).collect();
                Heightmap::new(v, 15, 15)
            },
            dem_source: self.source,
            heightmap_resolution,
            max_texture_present_level: max_texture_present_level as u8,
            max_dem_level: max_dem_level as u8,
            skirt,
            directory_name: format!("maps/t.{}/", self.name()),
            mapfile,
        };

        context.set_progress_and_total(0, 2);
        state.generate_heightmaps(context)?;
        context.set_progress(1);
        state.generate_colormaps(context)?;
        context.set_progress(2);

        Ok(tile_header)
    }
}

struct State {
    dem_source: DemSource,

    random: Heightmap<f32>,

    /// Resolution of the intermediate heightmaps which are used to generate normalmaps and
    /// colormaps. Derived from the target texture resolution.
    heightmap_resolution: u16,

    skirt: u16,

    max_texture_present_level: u8,
    max_dem_level: u8,

    directory_name: String,
    mapfile: MapFile,
}

impl State {
    fn generate_heightmaps(&mut self, context: &mut AssetLoadContext) -> Result<(), Error> {
        let missing = self.mapfile.get_missing_base(LayerType::Heightmaps)?;
        if missing.is_empty() {
            return Ok(());
        }

        // let _global_dem = GlobalDem.load(context)?;

        // let dem_cache = Rc::new(RefCell::new(RasterCache::new(Box::new(self.dem_source), 128)));
        // let reproject = ReprojectedDemDef {
        //     name: format!("{}dem", self.directory_name),
        //     dem_cache,
        //     nodes: &missing,
        //     random: &self.random,
        //     skirt: self.skirt,
        //     max_dem_level: self.max_dem_level as u8,
        //     max_texture_present_level: self.max_texture_present_level as u8,
        //     resolution: self.heightmap_resolution,
        //     global_dem,
        // };
        // let heightmaps = ReprojectedRaster::from_dem(reproject, context)?;

        context.increment_level("Writing heightmaps... ", missing.len());
        for (i, n) in missing.into_iter().enumerate() {
            context.set_progress(i as u64);
            let mut heightmap = Vec::new();
            for y in 0..self.heightmap_resolution {
                for x in 0..self.heightmap_resolution {
                    heightmap.write_f32::<LittleEndian>(0.0)?;
                }
            }
            self.mapfile.write_tile(LayerType::Heightmaps, n, &heightmap, true)?;
        }
        context.decrement_level();

        Ok(())
    }

    fn generate_colormaps(&mut self, context: &mut AssetLoadContext) -> Result<(), Error> {
        assert!(self.skirt >= 2);
        let colormap_resolution = self.heightmap_resolution - 5;

        let missing = self.mapfile.get_missing_base(LayerType::Albedo)?;
        if missing.is_empty() {
            return Ok(());
        }

        context.increment_level("Generating colormaps... ", missing.len());

        // let heights = self.heightmaps.as_ref().unwrap();

        // let reproject_bluemarble = ReprojectedRasterDef {
        //     name: format!("{}bluemarble", self.directory_name),
        //     nodes: &missing[..],
        //     resolution: colormap_resolution,
        //     skirt: self.skirt,
        //     datatype: DataType::U8,
        //     raster: RasterSource::Hybrid {
        //         global: Box::new(BlueMarble),
        //         cache: Rc::new(RefCell::new(RasterCache::new(Box::new(BlueMarbleTileSource), 8))),
        //     },
        // };
        // let bluemarble = ReprojectedRaster::from_raster(reproject_bluemarble, context).unwrap();

        let bluemarble = BlueMarble.load(context)?;
        let bluemarble_spacing = bluemarble.spacing() as f32;

        // let reproject = ReprojectedRasterDef {
        //     name: format!("{}watermasks", self.directory_name),
        //     heights: self.heightmaps.as_ref().unwrap(),
        //     system: &self.system,
        //     nodes: &self.nodes,
        //     skirt: self.skirt,
        //     datatype: DataType::U8,
        //     raster: RasterSource::GlobalRaster::<_, BitContainer> {
        //         global: Box::new(GlobalWaterMask),
        //     },
        // };
        // let watermasks = ReprojectedRaster::from_raster(reproject, context)?;

        for (i, n) in missing.into_iter().enumerate() {
            context.set_progress(i as u64);

            let mut colormap =
                Vec::with_capacity(colormap_resolution as usize * colormap_resolution as usize);
            // let _heights = self.heightmaps.as_ref().unwrap();
            let spacing =
                n.aprox_side_length() / (self.heightmap_resolution - 2 * self.skirt) as f32;

            if spacing <= bluemarble_spacing {
                self.mapfile.set_missing(LayerType::Albedo, n, false)?;
                continue;
            }

            for y in 0..colormap_resolution {
                for x in 0..colormap_resolution {
                    let cspace =
                        n.cell_position_cspace(x as i32, y as i32, self.skirt, colormap_resolution);
                    let sspace = CoordinateSystem::cspace_to_sspace(cspace);
                    let polar = CoordinateSystem::sspace_to_polar(sspace);
                    let (lat, long) = (polar.x.to_degrees(), polar.y.to_degrees());

                    let color = {
                        let r = bluemarble.interpolate(lat, long, 0) as u8;
                        let g = bluemarble.interpolate(lat, long, 0) as u8;
                        let b = bluemarble.interpolate(lat, long, 0) as u8;
                        let roughness = (0.7 * 255.0) as u8;
                        [SRGB_TO_LINEAR[r], SRGB_TO_LINEAR[g], SRGB_TO_LINEAR[b], roughness]
                    };

                    colormap.extend_from_slice(&color);
                }
            }

            self.mapfile.write_tile(LayerType::Albedo, n, &colormap, true)?;
        }

        context.decrement_level();
        Ok(())
    }

    fn generate_noise(_context: &mut AssetLoadContext) -> Result<NoiseParams, Error> {
        let noise = NoiseParams {
            texture: TextureDescriptor {
                resolution: 2048,
                format: TextureFormat::RGBA8,
                bytes: 4 * 2048 * 2048,
            },
            wavelength: 1.0 / 256.0,
        };

        let noise_heightmaps: Vec<_> =
            (0..4).map(|i| heightmap::wavelet_noise(64 << i, 32 >> i)).collect();

        let len = noise_heightmaps[0].heights.len();
        let mut heights = vec![0u8; len * 4];
        for (i, heightmap) in noise_heightmaps.into_iter().enumerate() {
            let mut dist: Vec<(usize, f32)> = heightmap.heights.into_iter().enumerate().collect();
            dist.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
            for j in 0..len {
                heights[dist[j].0 * 4 + i] = (j * 256 / len) as u8;
            }
        }

        MapFile::save_noise_texture(&noise.texture, &heights[..])?;
        Ok(noise)
    }
}
