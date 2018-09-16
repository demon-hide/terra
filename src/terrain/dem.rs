use std::io::{Cursor, Read};
use std::mem;
use std::str::FromStr;

use failure::Error;
use image::tiff::TIFFDecoder;
use image::{DecodingResult, ImageDecoder};
use safe_transmute;
use zip::ZipArchive;

use cache::{AssetLoadContext, WebAsset};
use terrain::raster::{GlobalRaster, Raster, RasterSource};

#[derive(Debug, Fail)]
#[fail(display = "failed to parse DEM file")]
pub struct DemParseError;

/// Which data source to use for digital elevation models.
#[derive(Copy, Clone)]
pub enum DemSource {
    /// Use DEMs from the USGS National Map at approximately 30 meters. Data from this source is
    /// only available for the United States.
    Usgs30m,
    /// Use DEMs from the USGS National Map at approximately 30 meters. Data from this source is
    /// only available for the United States.
    Usgs10m,
    /// Use DEMs Shuttle Radar Topography Mission (SRTM) 1 Arc-Second Global data source. Data is
    /// available globally between 60° north and 56° south latitude.
    Srtm30m,
}
impl DemSource {
    pub(crate) fn url_str(&self) -> &str {
        match *self {
            DemSource::Usgs30m => {
                "https://prd-tnm.s3.amazonaws.com/StagedProducts/Elevation/1/GridFloat/"
            }
            DemSource::Usgs10m => {
                "https://prd-tnm.s3.amazonaws.com/StagedProducts/Elevation/13/GridFloat/"
            }
            DemSource::Srtm30m => {
                "https://cloud.sdsc.edu/v1/AUTH_opentopography/Raster/SRTM_GL1/SRTM_GL1_srtm/"
            }
        }
    }
    pub(crate) fn directory_str(&self) -> &str {
        match *self {
            DemSource::Usgs30m => "dems/ned1",
            DemSource::Usgs10m => "dems/ned13",
            DemSource::Srtm30m => "dems/srtm1",
        }
    }
    /// Returns the approximate resolution of data from this source in meters.
    pub(crate) fn resolution(&self) -> u32 {
        match *self {
            DemSource::Usgs30m => 30,
            DemSource::Usgs10m => 10,
            DemSource::Srtm30m => 30,
        }
    }
    /// Returns the size of cells from this data source in arcseconds.
    pub(crate) fn cell_size(&self) -> f32 {
        match *self {
            DemSource::Usgs30m => 1.0,
            DemSource::Usgs10m => 1.0 / 3.0,
            DemSource::Srtm30m => 1.0,
        }
    }
}
impl RasterSource for DemSource {
    type Type = f32;
    fn load(
        &self,
        context: &mut AssetLoadContext,
        latitude: i16,
        longitude: i16,
    ) -> Option<Raster<f32>> {
        DigitalElevationModelParams {
            latitude,
            longitude,
            source: *self,
        }.load(context)
        .ok()
    }
}

pub struct DigitalElevationModelParams {
    pub latitude: i16,
    pub longitude: i16,
    pub source: DemSource,
}
impl WebAsset for DigitalElevationModelParams {
    type Type = Raster<f32>;

    fn url(&self) -> String {
        let (latitude, longitude) = match self.source {
            DemSource::Usgs30m | DemSource::Usgs10m => (self.latitude + 1, self.longitude),
            _ => (self.latitude, self.longitude),
        };

        let n_or_s = if latitude >= 0 { 'n' } else { 's' };
        let e_or_w = if longitude >= 0 { 'e' } else { 'w' };

        match self.source {
            DemSource::Usgs30m | DemSource::Usgs10m => format!(
                "{}{}{:02}{}{:03}.zip",
                self.source.url_str(),
                n_or_s,
                latitude.abs(),
                e_or_w,
                longitude.abs()
            ),
            DemSource::Srtm30m => format!(
                "{}{}/{}{:02}{}{:03}.hgt",
                self.source.url_str(),
                if latitude >= 30 {
                    "North/North_30_60"
                } else if latitude >= 0 {
                    "North/North_0_29"
                } else {
                    "South"
                },
                n_or_s.to_uppercase().next().unwrap(),
                latitude.abs(),
                e_or_w.to_uppercase().next().unwrap(),
                longitude.abs()
            ),
        }
    }
    fn filename(&self) -> String {
        let n_or_s = if self.latitude >= 0 { 'n' } else { 's' };
        let e_or_w = if self.longitude >= 0 { 'e' } else { 'w' };
        format!(
            "{}/{}{:02}_{}{:03}.zip",
            self.source.directory_str(),
            n_or_s,
            self.latitude.abs(),
            e_or_w,
            self.longitude.abs()
        )
    }
    fn parse(&self, _context: &mut AssetLoadContext, data: Vec<u8>) -> Result<Self::Type, Error> {
        match self.source {
            DemSource::Usgs30m | DemSource::Usgs10m => parse_ned_zip(data),
            DemSource::Srtm30m => parse_srtm1_hgt(self.latitude, self.longitude, data),
        }
    }
}

/// Load a zip file in the format for the USGS's National Elevation Dataset.
fn parse_ned_zip(data: Vec<u8>) -> Result<Raster<f32>, Error> {
    let mut hdr = String::new();
    let mut flt = Vec::new();

    let mut zip = ZipArchive::new(Cursor::new(data))?;
    for i in 0..zip.len() {
        let mut file = zip.by_index(i)?;
        if file.name().ends_with(".hdr") {
            assert_eq!(hdr.len(), 0);
            file.read_to_string(&mut hdr)?;
        } else if file.name().ends_with(".flt") {
            assert_eq!(flt.len(), 0);
            file.read_to_end(&mut flt)?;
        }
    }

    enum ByteOrder {
        LsbFirst,
        MsbFirst,
    }

    let mut width = None;
    let mut height = None;
    let mut xllcorner = None;
    let mut yllcorner = None;
    let mut cell_size = None;
    let mut byte_order = None;
    let mut nodata_value = None;
    for line in hdr.lines() {
        let mut parts = line.split_whitespace();
        let key = parts.next();
        let value = parts.next();
        if let (Some(key), Some(value)) = (key, value) {
            match key {
                "ncols" => width = usize::from_str(value).ok(),
                "nrows" => height = usize::from_str(value).ok(),
                "xllcorner" => xllcorner = f64::from_str(value).ok(),
                "yllcorner" => yllcorner = f64::from_str(value).ok(),
                "cellsize" => cell_size = f64::from_str(value).ok(),
                "NODATA_value" => nodata_value = f32::from_str(value).ok(),
                "byteorder" => {
                    byte_order = match value {
                        "LSBFIRST" => Some(ByteOrder::LsbFirst),
                        "MSBFIRST" => Some(ByteOrder::MsbFirst),
                        _ => panic!("unrecognized byte order: {}", value),
                    }
                }
                _ => {}
            }
        }
    }

    let width = width.ok_or(DemParseError)?;
    let height = height.ok_or(DemParseError)?;
    let xllcorner = xllcorner.ok_or(DemParseError)?;
    let yllcorner = yllcorner.ok_or(DemParseError)?;
    let cell_size = cell_size.ok_or(DemParseError)?;
    let byte_order = byte_order.ok_or(DemParseError)?;
    let nodata_value = nodata_value.ok_or(DemParseError)?;

    let size = width * height;
    if flt.len() != size * 4 {
        Err(DemParseError)?;
    }

    let flt = unsafe { safe_transmute::guarded_transmute_many_pedantic::<u32>(&flt[..]).unwrap() };
    let mut elevations: Vec<f32> = Vec::with_capacity(size);
    for f in flt {
        let e = match byte_order {
            ByteOrder::LsbFirst => f.to_le(),
            ByteOrder::MsbFirst => f.to_be(),
        };
        let e = unsafe { mem::transmute::<u32, f32>(e) };
        elevations.push(if e == nodata_value { 0.0 } else { e });
    }

    Ok(Raster {
        width,
        height,
        xllcorner,
        yllcorner,
        cell_size,
        values: elevations,
    })
}

/// Load a HGT file in the format for the NASA's STRM 30m dataset.
fn parse_srtm1_hgt(latitude: i16, longitude: i16, hgt: Vec<u8>) -> Result<Raster<f32>, Error> {
    let resolution = 3601;
    let cell_size = 1.0 / 3600.0;

    if hgt.len() != resolution * resolution * 2 {
        bail!(DemParseError);
    }

    let hgt = unsafe { safe_transmute::guarded_transmute_many_pedantic::<i16>(&hgt[..]).unwrap() };
    let mut elevations: Vec<f32> = Vec::with_capacity(resolution * resolution);

    for x in 0..resolution {
        for y in 0..resolution {
            let h = i16::from_be(hgt[(resolution - x - 1) + (resolution - y - 1) * resolution]);
            if h == -32768 {
                elevations.push(0.0);
            } else {
                elevations.push(h as f32);
            }
        }
    }

    Ok(Raster {
        width: resolution,
        height: resolution,
        xllcorner: latitude as f64,
        yllcorner: longitude as f64,
        cell_size,
        values: elevations,
    })
}

pub struct GlobalDem;
impl WebAsset for GlobalDem {
    type Type = GlobalRaster<i16>;

    fn url(&self) -> String {
        "https://www.ngdc.noaa.gov/mgg/global/relief/ETOPO1/data/ice_surface/cell_registered/georeferenced_tiff/ETOPO1_Ice_c_geotiff.zip".to_string()
    }
    fn filename(&self) -> String {
        "dems/ETOPO1_Ice_c_geotiff.zip".to_string()
    }
    fn parse(&self, _context: &mut AssetLoadContext, data: Vec<u8>) -> Result<Self::Type, Error> {
        let mut zip = ZipArchive::new(Cursor::new(data))?;
        ensure!(zip.len() == 1, "Unexpected zip file contents");
        let mut file = zip.by_index(0)?;
        ensure!(
            file.name() == "ETOPO1_Ice_c_geotiff.tif",
            "Unexpected zip file contents"
        );

        let mut contents = Vec::new();
        file.read_to_end(&mut contents)?;

        let mut tiff_decoder = TIFFDecoder::new(Cursor::new(contents))?;
        let (width, height) = tiff_decoder.dimensions()?;
        match tiff_decoder.read_image()? {
            DecodingResult::U8(_) => bail!("unexpected format"),
            DecodingResult::U16(values) => Ok(GlobalRaster {
                bands: 1,
                width: width as usize,
                height: height as usize,
                values: values.into_iter().map(|i| i as i16).collect(),
            }),
        }
    }
}
