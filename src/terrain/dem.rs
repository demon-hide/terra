use zip::ZipArchive;
use safe_transmute;

use std::error::Error;
use std::fmt::{self, Display};
use std::io::{Cursor, Read};
use std::str::FromStr;
use std::mem;

use cache::WebAsset;

#[derive(Debug)]
pub enum DemError {
    ParseError,
}
impl Error for DemError {
    fn description(&self) -> &str {
        "failed to parse DEM"
    }
}
impl Display for DemError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.description())
    }
}

#[derive(Copy, Clone)]
pub enum DemSource {
    Usgs30m,
    Usgs10m,
}
impl DemSource {
    pub(crate) fn as_str(&self) -> &str {
        match *self {
            DemSource::Usgs30m => "1",
            DemSource::Usgs10m => "13",
        }
    }
    pub(crate) fn resolution(&self) -> u32 {
        match *self {
            DemSource::Usgs30m => 30,
            DemSource::Usgs10m => 10,
        }
    }
}

pub struct DigitalElevationModelParams {
    pub latitude: i16,
    pub longitude: i16,
    pub source: DemSource,
}
impl WebAsset for DigitalElevationModelParams {
    type Type = DigitalElevationModel;

    fn url(&self) -> String {
        let n_or_s = if self.latitude >= 0 { 'n' } else { 's' };
        let e_or_w = if self.longitude >= 0 { 'e' } else { 'w' };
        format!(
            "https://prd-tnm.s3.amazonaws.com/StagedProducts/Elevation/{}/GridFloat/\
                       USGS_NED_{}_{}{:02}{}{:03}_GridFloat.zip",
            self.source.as_str(),
            self.source.as_str(),
            n_or_s,
            self.latitude.abs(),
            e_or_w,
            self.longitude.abs()
        )
    }
    fn filename(&self) -> String {
        let n_or_s = if self.latitude >= 0 { 'n' } else { 's' };
        let e_or_w = if self.longitude >= 0 { 'e' } else { 'w' };
        format!(
            "dems/ned{}/{}{:02}_{}{:03}_GridFloat.zip",
            self.source.as_str(),
            n_or_s,
            self.latitude.abs(),
            e_or_w,
            self.longitude.abs()
        )
    }
    fn parse(&self, data: Vec<u8>) -> Result<Self::Type, Box<::std::error::Error>> {
        let mut hdr = String::new();
        let mut flt = Vec::new();

        let mut zip = ZipArchive::new(Cursor::new(data))?;
        for i in 0..zip.len() {
            let mut file = zip.by_index(i)?;
            if file.name().ends_with("_gridfloat.hdr") {
                assert_eq!(hdr.len(), 0);
                file.read_to_string(&mut hdr)?;
            } else if file.name().ends_with("_gridfloat.flt") {
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

        let width = width.ok_or(DemError::ParseError)?;
        let height = height.ok_or(DemError::ParseError)?;
        let xllcorner = xllcorner.ok_or(DemError::ParseError)?;
        let yllcorner = yllcorner.ok_or(DemError::ParseError)?;
        let cell_size = cell_size.ok_or(DemError::ParseError)?;
        let byte_order = byte_order.ok_or(DemError::ParseError)?;

        let size = width * height;
        if flt.len() != size * 4 {
            return Err(Box::new(DemError::ParseError));
        }

        let flt =
            unsafe { safe_transmute::guarded_transmute_many_pedantic::<u32>(&flt[..]).unwrap() };
        let mut elevations: Vec<f32> = Vec::with_capacity(size);
        for f in flt {
            let e = match byte_order {
                ByteOrder::LsbFirst => f.to_le(),
                ByteOrder::MsbFirst => f.to_be(),
            };
            elevations.push(unsafe { mem::transmute::<u32, f32>(e) });
        }

        Ok(Self::Type {
            width,
            height,
            xllcorner,
            yllcorner,
            cell_size,
            elevations,
        })

    }
}

pub struct DigitalElevationModel {
    pub width: usize,
    pub height: usize,
    pub cell_size: f64,

    pub xllcorner: f64,
    pub yllcorner: f64,

    pub elevations: Vec<f32>,
}

impl DigitalElevationModel {
    pub fn crop(&self, width: usize, height: usize) -> Self {
        assert!(width > 0 && width <= self.width);
        assert!(height > 0 && height <= self.height);

        let xoffset = (self.width - width) / 2;
        let yoffset = (self.height - height) / 2;

        let mut elevations = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                elevations.push(self.elevations[(x + xoffset) + (y + yoffset) * self.width]);
            }
        }

        Self {
            width,
            height,
            cell_size: self.cell_size,
            xllcorner: self.xllcorner + self.cell_size * (xoffset as f64),
            yllcorner: self.yllcorner + self.cell_size * (yoffset as f64),
            elevations,
        }
    }
}

#[cfg(test)]
mod tests {
    // #[test]
    // fn it_works() {
    //     use super::*;

    //     let zip = Dem::download_gridfloat_zip(28, -81, DemSource::Usgs30m);
    //     let dem = Dem::from_gridfloat_zip(Cursor::new(zip));
    //     assert_eq!(dem.width, 3612);
    //     assert_eq!(dem.height, 3612);
    //     assert!(dem.cell_size > 0.0002777);
    //     assert!(dem.cell_size < 0.0002778);
    // }
}
