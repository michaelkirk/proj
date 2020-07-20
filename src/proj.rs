use geo_types::Point;
use libc::c_int;
use libc::{c_char, c_double};
use num_traits::Float;
use proj_sys::{
    proj_area_create, proj_area_destroy, proj_area_set_bbox, proj_cleanup, proj_context_create,
    proj_context_destroy, proj_context_get_url_endpoint, proj_context_is_network_enabled,
    proj_context_set_enable_network, proj_context_set_search_paths, proj_context_set_url_endpoint,
    proj_create, proj_create_crs_to_crs, proj_destroy, proj_errno_string,
    proj_grid_cache_set_enable, proj_info, proj_normalize_for_visualization, proj_pj_info,
    proj_trans, proj_trans_array, PJconsts, PJ_AREA, PJ_CONTEXT, PJ_COORD, PJ_DIRECTION_PJ_FWD,
    PJ_DIRECTION_PJ_INV, PJ_INFO, PJ_LP, PJ_XY,
};

use crate::network::set_network_callbacks;
use proj_sys::{proj_errno, proj_errno_reset};

use std::ffi::CStr;
use std::ffi::CString;
use std::str;
use std::{path::Path, ptr};
use thiserror::Error;

/// Errors originating in PROJ which can occur during projection and conversion
#[derive(Error, Debug)]
pub enum ProjError {
    /// A projection error
    #[error("The projection failed with the following error: {0}")]
    Projection(String),
    /// A conversion error
    #[error("The conversion failed with the following error: {0}")]
    Conversion(String),
    /// An error that occurs when a path string originating in PROJ can't be converted to a CString
    #[error("Couldn't create a raw pointer from the string")]
    Creation(#[from] std::ffi::NulError),
    /// An error that occurs if a user-supplied path can't be converted into a string slice
    #[error("Couldn't convert path to slice")]
    Path,
    #[error("Couldn't convert bytes from PROJ to UTF-8")]
    Utf8Error(#[from] std::str::Utf8Error),
    #[error("Couldn't convert number to f64")]
    FloatConversion,
    #[error("Network download functionality could not be enabled")]
    Network,
    #[error("Could not set remote grid download callbacks")]
    RemoteCallbacks,
    #[error("Couldn't build request")]
    BuilderError(#[from] reqwest::Error),
    #[error("Couldn't clone request")]
    RequestCloneError,
    #[error("Could not retrieve content length")]
    ContentLength,
    #[error("Couldn't retrieve header for key {0}")]
    HeaderError(String),
    #[error("Couldn't convert header value to str")]
    HeaderConversion(#[from] reqwest::header::ToStrError),
    #[error("A {0} error occurred for url {1} after {2} retries")]
    DownloadError(String, String, u8),
}

/// The bounding box of an area of use
///
/// In the case of an area of use crossing the antimeridian (longitude +/- 180 degrees),
/// `west` must be greater than `east`.
#[derive(Copy, Clone, Debug)]
pub struct Area {
    north: f64,
    south: f64,
    east: f64,
    west: f64,
}

impl Area {
    /// Create a new Area
    ///
    /// **Note**: In the case of an area of use crossing the antimeridian (longitude +/- 180 degrees),
    /// `west` must be greater than `east`.
    pub fn new(west: f64, south: f64, east: f64, north: f64) -> Self {
        Area {
            west,
            south,
            east,
            north,
        }
    }
}

/// Easily get a String from the external library
pub(crate) fn _string(raw_ptr: *const c_char) -> Result<String, ProjError> {
    let c_str = unsafe { CStr::from_ptr(raw_ptr) };
    Ok(str::from_utf8(c_str.to_bytes())?.to_string())
}

/// Look up an error message using the error code
fn error_message(code: c_int) -> Result<String, ProjError> {
    let rv = unsafe { proj_errno_string(code) };
    _string(rv)
}

/// Set the bounding box of the area of use
fn area_set_bbox(parea: *mut proj_sys::PJ_AREA, new_area: Option<Area>) {
    // if a bounding box has been passed, modify the proj area object
    if let Some(narea) = new_area {
        unsafe {
            proj_area_set_bbox(parea, narea.west, narea.south, narea.east, narea.north);
        }
    }
}

/// Enable or disable network access for [resource file download](https://proj.org/resource_files.html#where-are-proj-resource-files-looked-for).
///
/// This will configure network access for all **subsequent** `Proj` instances, but will **not** affect pre-existing instances.
/// # Safety
/// This method contains unsafe code.
pub fn enable_network(enable: bool) -> Result<u8, ProjError> {
    if enable {
        let _ = match set_network_callbacks() {
            1 => Ok(1),
            _ => Err(ProjError::Network),
        }?;
    }
    let enable = if enable { 1 } else { 0 };
    let dctx: *mut PJ_CONTEXT = ptr::null_mut();
    match unsafe { proj_context_set_enable_network(dctx, enable) } {
        1 => Ok(1),
        _ => Err(ProjError::Network),
    }
}

/// Check whether network access for [resource file download](https://proj.org/resource_files.html#where-are-proj-resource-files-looked-for) is currently enabled or disabled.
///
/// # Safety
/// This method contains unsafe code.
pub fn network_enabled() -> bool {
    let dctx: *mut PJ_CONTEXT = ptr::null_mut();
    let res = unsafe { proj_context_is_network_enabled(dctx) };
    match res {
        1 => true,
        _ => false,
    }
}

/// Enable or disable the local cache of grid chunks for all subsequent PROJ instances
///
/// To avoid repeated network access, a local cache of downloaded chunks of grids is
/// implemented as SQLite3 database, cache.db, stored in the PROJ user writable directory.
/// This local caching is **enabled** by default.
/// The default maximum size of the cache is 300 MB, which is more than half of the total size
/// of grids available, at time of writing.
///
/// # Safety
/// This method contains unsafe code.
pub fn grid_cache_set_enable(enable: bool) {
    let enable = if enable { 1 } else { 0 };
    let dctx: *mut PJ_CONTEXT = ptr::null_mut();
    let _ = unsafe { proj_grid_cache_set_enable(dctx, enable) };
}

/// Get the URL endpoint to query for remote grids
///
/// # Safety
/// This method contains unsafe code.
pub fn get_url_endpoint() -> Result<String, ProjError> {
    let dctx: *mut PJ_CONTEXT = ptr::null_mut();
    unsafe { _string(proj_context_get_url_endpoint(dctx)) }
}

/// Set the URL endpoint to query for remote grids for all subsequent PROJ instances
///
/// # Safety
/// This method contains unsafe code.
pub fn set_url_endpoint(endpoint: &str) -> Result<(), ProjError> {
    let s = CString::new(endpoint)?;
    let dctx: *mut PJ_CONTEXT = ptr::null_mut();
    unsafe { proj_context_set_url_endpoint(dctx, s.as_ptr()) };
    Ok(())
}

enum Transformation {
    Projection,
    Conversion,
}

/// [Information](https://proj.org/development/reference/datatypes.html#c.PJ_INFO) about the current PROJ context
#[derive(Clone, Debug)]
pub struct Projinfo {
    pub major: i32,
    pub minor: i32,
    pub patch: i32,
    pub release: String,
    pub version: String,
    pub searchpath: String,
}

/// A `PROJ` instance
pub struct Proj {
    c_proj: *mut PJconsts,
    ctx: *mut PJ_CONTEXT,
    area: Option<*mut PJ_AREA>,
}

impl Proj {
    /// Try to instantiate a new `PROJ` instance
    ///
    /// **Note:** for projection operations, `definition` specifies
    /// the **output** projection; input coordinates
    /// are assumed to be geodetic in radians, unless an inverse projection is intended.
    ///
    /// For conversion operations, `definition` defines input, output, and
    /// any intermediate steps that are required. See the `convert` example for more details.
    ///
    /// # Safety
    /// This method contains unsafe code.

    // In contrast to proj v4.x, the type of transformation
    // is signalled by the choice of enum used as input to the PJ_COORD union
    // PJ_LP signals projection of geodetic coordinates, with output being PJ_XY
    // and vice versa, or using PJ_XY for conversion operations
    pub fn new(definition: &str) -> Option<Proj> {
        let c_definition = CString::new(definition).ok()?;
        let ctx = unsafe { proj_context_create() };
        let new_c_proj = unsafe { proj_create(ctx, c_definition.as_ptr()) };
        if new_c_proj.is_null() {
            None
        } else {
            Some(Proj {
                c_proj: new_c_proj,
                ctx,
                area: None,
            })
        }
    }

    /// Create a transformation object that is a pipeline between two known coordinate reference systems.
    /// `from` and `to` can be:
    ///
    /// - an `"AUTHORITY:CODE"`, like `"EPSG:25832"`.
    /// - a PROJ string, like `"+proj=longlat +datum=WGS84"`. When using that syntax, the unit is expected to be degrees.
    /// - the name of a CRS as found in the PROJ database, e.g `"WGS84"`, `"NAD27"`, etc.
    /// - more generally, any string accepted by [`new()`](struct.Proj.html#method.new)
    ///
    /// If you wish to alter the particular area of use, you may do so using [`area_set_bbox()`](struct.Proj.html#method.area_set_bbox)
    /// ## A Note on Coordinate Order
    /// The required input **and** output coordinate order is **normalised** to `Longitude, Latitude` / `Easting, Northing`.
    ///
    /// This overrides the expected order of the specified input and / or output CRS if necessary.
    /// See the [PROJ API](https://proj.org/development/reference/functions.html#c.proj_normalize_for_visualization)
    ///
    /// For example: per its definition, EPSG:4326 has an axis order of Latitude, Longitude. Without
    /// normalisation, crate users would have to
    /// [remember](https://proj.org/development/reference/functions.html#c.proj_create_crs_to_crs)
    /// to reverse the coordinates of `Point` or `Coordinate` structs in order for a conversion operation to
    /// return correct results.
    ///
    ///```rust
    /// # use assert_approx_eq::assert_approx_eq;
    /// extern crate proj;
    /// use proj::Proj;
    ///
    /// extern crate geo_types;
    /// use geo_types::Point;
    ///
    /// let from = "EPSG:2230";
    /// let to = "EPSG:26946";
    /// let nad_ft_to_m = Proj::new_known_crs(&from, &to, None).unwrap();
    /// let result = nad_ft_to_m
    ///     .convert(Point::new(4760096.421921f64, 3744293.729449f64))
    ///     .unwrap();
    /// assert_approx_eq!(result.x(), 1450880.29f64, 1.0e-2);
    /// assert_approx_eq!(result.y(), 1141263.01f64, 1.0e-2);
    /// ```
    ///
    /// # Safety
    /// This method contains unsafe code.
    pub fn new_known_crs(from: &str, to: &str, area: Option<Area>) -> Option<Proj> {
        let from_c = CString::new(from).ok()?;
        let to_c = CString::new(to).ok()?;
        let ctx = unsafe { proj_context_create() };
        let proj_area = unsafe { proj_area_create() };
        area_set_bbox(proj_area, area);
        let new_c_proj =
            unsafe { proj_create_crs_to_crs(ctx, from_c.as_ptr(), to_c.as_ptr(), proj_area) };
        if new_c_proj.is_null() {
            None
        } else {
            // Normalise input and output order to Lon, Lat / Easting Northing by inserting
            // An axis swap operation if necessary
            let normalised = unsafe {
                let normalised = proj_normalize_for_visualization(ctx, new_c_proj);
                // deallocate stale PJ pointer
                proj_destroy(new_c_proj);
                normalised
            };
            Some(Proj {
                c_proj: normalised,
                ctx,
                area: Some(proj_area),
            })
        }
    }

    /// Return [Information](https://proj.org/development/reference/datatypes.html#c.PJ_INFO) about the current PROJ context
    ///
    /// # Safety
    /// This method contains unsafe code.
    pub fn info(&self) -> Result<Projinfo, ProjError> {
        let pinfo: PJ_INFO = unsafe { proj_info() };
        Ok(Projinfo {
            major: pinfo.major,
            minor: pinfo.minor,
            patch: pinfo.patch,
            release: _string(pinfo.release)?,
            version: _string(pinfo.version)?,
            searchpath: _string(pinfo.searchpath)?,
        })
    }

    /// Add a [resource file search path](https://proj.org/resource_files.html), maintaining existing entries.
    ///
    /// Changes to the search path [_should be_](https://github.com/OSGeo/PROJ/issues/2266) reflected in this
    /// and **all** subsequently-created `Proj` instances, but **not** in other concurrently-existing instances.
    ///
    /// # Safety
    /// This method contains unsafe code.
    pub fn set_search_paths<P: AsRef<Path>>(&self, newpath: P) -> Result<(), ProjError> {
        let existing = self.info()?.searchpath;
        let pathsep = if cfg!(windows) { ";" } else { ":" };
        let mut individual: Vec<&str> = existing.split(pathsep).collect();
        let np = Path::new(newpath.as_ref());
        individual.push(np.to_str().ok_or(ProjError::Path)?);
        let newlength = individual.len() as i32;
        // convert path entries to CString
        let paths_c = individual
            .iter()
            .map(|str| CString::new(*str))
            .collect::<Result<Vec<_>, std::ffi::NulError>>()?;
        // …then to raw pointers
        let paths_p: Vec<_> = paths_c.iter().map(|cstr| cstr.as_ptr()).collect();
        // …then pass the slice of raw pointers as a raw pointer (const char* const*)
        // We pass a null pointer as the context, as we want the search path to be
        // available to all contexts
        let dctx: *mut PJ_CONTEXT = ptr::null_mut();
        unsafe { proj_context_set_search_paths(self.ctx, newlength, paths_p.as_ptr()) }
        unsafe { proj_context_set_search_paths(dctx, newlength, paths_p.as_ptr()) }
        Ok(())
    }

    /// Set the bounding box of the area of use
    ///
    /// This bounding box will be used to specify the area of use
    /// for the choice of relevant coordinate operations.
    /// In the case of an area of use crossing the antimeridian (longitude +/- 180 degrees),
    /// `west` **must** be greater than `east`.
    ///
    /// # Safety
    /// This method contains unsafe code.
    // calling this on a non-CRS-to-CRS instance of Proj will be harmless, because self.area will be None
    pub fn area_set_bbox(&mut self, new_bbox: Area) {
        if let Some(new_area) = self.area {
            unsafe {
                proj_area_set_bbox(
                    new_area,
                    new_bbox.west,
                    new_bbox.south,
                    new_bbox.east,
                    new_bbox.north,
                );
            }
        }
    }

    /// Get the current definition from `PROJ`
    ///
    /// # Safety
    /// This method contains unsafe code.
    pub fn def(&self) -> Result<String, ProjError> {
        let rv = unsafe { proj_pj_info(self.c_proj) };
        _string(rv.definition)
    }

    /// Project geodetic coordinates (in radians) into the projection specified by `definition`
    ///
    /// **Note:** specifying `inverse` as `true` carries out an inverse projection *to* geodetic coordinates
    /// (in radians) from the projection specified by `definition`.
    ///
    /// # Safety
    /// This method contains unsafe code.
    pub fn project<T, U>(&self, point: T, inverse: bool) -> Result<Point<U>, ProjError>
    where
        T: Into<Point<U>>,
        U: Float,
    {
        let inv = if inverse {
            PJ_DIRECTION_PJ_INV
        } else {
            PJ_DIRECTION_PJ_FWD
        };
        let _point: Point<U> = point.into();
        let c_x: c_double = _point.x().to_f64().ok_or(ProjError::FloatConversion)?;
        let c_y: c_double = _point.y().to_f64().ok_or(ProjError::FloatConversion)?;
        let new_x;
        let new_y;
        let err;
        // Input coords are defined in terms of lambda & phi, using the PJ_LP struct.
        // This signals that we wish to project geodetic coordinates.
        // For conversion (i.e. between projected coordinates) you should use
        // PJ_XY {x: , y: }
        let coords = PJ_LP { lam: c_x, phi: c_y };
        unsafe {
            proj_errno_reset(self.c_proj);
            // PJ_DIRECTION_* determines a forward or inverse projection
            let trans = proj_trans(self.c_proj, inv, PJ_COORD { lp: coords });
            // output of coordinates uses the PJ_XY struct
            new_x = trans.xy.x;
            new_y = trans.xy.y;
            err = proj_errno(self.c_proj);
        }
        if err == 0 {
            Ok(Point::new(
                U::from(new_x).ok_or(ProjError::FloatConversion)?,
                U::from(new_y).ok_or(ProjError::FloatConversion)?,
            ))
        } else {
            Err(ProjError::Projection(error_message(err)?))
        }
    }

    /// Convert projected coordinates between coordinate reference systems.
    ///
    /// Input and output CRS may be specified in two ways:
    /// 1. Using the PROJ `pipeline` operator. This method makes use of the [`pipeline`](http://proj4.org/operations/pipeline.html)
    /// functionality available since `PROJ` 5.
    /// This has the advantage of being able to chain an arbitrary combination of projection, conversion,
    /// and transformation steps, allowing for extremely complex operations ([`new`](#method.new))
    /// 2. Using EPSG codes or `PROJ` strings to define input and output CRS ([`new_known_crs`](#method.new_known_crs))
    ///
    /// ## A Note on Coordinate Order
    /// Depending on the method used to instantiate the `Proj` object, coordinate input and output order may vary:
    /// - If you have used [`new`](#method.new), it is assumed that you've specified the order using the input string,
    /// or that you are aware of the required input order and expected output order.
    /// - If you have used [`new_known_crs`](#method.new_known_crs), input and output order are **normalised**
    /// to Longitude, Latitude / Easting, Northing.
    ///
    /// The following example converts from NAD83 US Survey Feet (EPSG 2230) to NAD83 Metres (EPSG 26946)
    ///
    /// ```rust
    /// # use assert_approx_eq::assert_approx_eq;
    /// extern crate proj;
    /// use proj::Proj;
    ///
    /// extern crate geo_types;
    /// use geo_types::Point;
    ///
    /// let from = "EPSG:2230";
    /// let to = "EPSG:26946";
    /// let ft_to_m = Proj::new_known_crs(&from, &to, None).unwrap();
    /// let result = ft_to_m
    ///     .convert(Point::new(4760096.421921, 3744293.729449))
    ///     .unwrap();
    /// assert_approx_eq!(result.x() as f64, 1450880.2910605003);
    /// assert_approx_eq!(result.y() as f64, 1141263.0111604529);
    /// ```
    ///
    /// # Safety
    /// This method contains unsafe code.
    pub fn convert<T, U>(&self, point: T) -> Result<Point<U>, ProjError>
    where
        T: Into<Point<U>>,
        U: Float,
    {
        let _point: Point<U> = point.into();
        let c_x: c_double = _point.x().to_f64().ok_or(ProjError::FloatConversion)?;
        let c_y: c_double = _point.y().to_f64().ok_or(ProjError::FloatConversion)?;
        let new_x;
        let new_y;
        let err;
        let coords = PJ_XY { x: c_x, y: c_y };
        unsafe {
            proj_errno_reset(self.c_proj);
            let trans = proj_trans(self.c_proj, PJ_DIRECTION_PJ_FWD, PJ_COORD { xy: coords });
            new_x = trans.xy.x;
            new_y = trans.xy.y;
            err = proj_errno(self.c_proj);
        }
        if err == 0 {
            Ok(Point::new(
                U::from(new_x).ok_or(ProjError::FloatConversion)?,
                U::from(new_y).ok_or(ProjError::FloatConversion)?,
            ))
        } else {
            Err(ProjError::Conversion(error_message(err)?))
        }
    }

    /// Convert a mutable slice (or anything that can deref into a mutable slice) of `Point`s
    ///
    /// The following example converts from NAD83 US Survey Feet (EPSG 2230) to NAD83 Metres (EPSG 26946)
    ///
    /// ## A Note on Coordinate Order
    /// Depending on the method used to instantiate the `Proj` object, coordinate input and output order may vary:
    /// - If you have used [`new`](#method.new), it is assumed that you've specified the order using the input string,
    /// or that you are aware of the required input order and expected output order.
    /// - If you have used [`new_known_crs`](#method.new_known_crs), input and output order are **normalised**
    /// to Longitude, Latitude / Easting, Northing.
    ///
    /// ```rust
    /// use proj::Proj;
    /// extern crate geo_types;
    /// use geo_types::Point;
    /// # use assert_approx_eq::assert_approx_eq;
    /// let from = "EPSG:2230";
    /// let to = "EPSG:26946";
    /// let ft_to_m = Proj::new_known_crs(&from, &to, None).unwrap();
    /// let mut v = vec![
    ///     Point::new(4760096.421921, 3744293.729449),
    ///     Point::new(4760197.421921, 3744394.729449),
    /// ];
    /// ft_to_m.convert_array(&mut v);
    /// assert_approx_eq!(v[0].x(), 1450880.2910605003f64);
    /// assert_approx_eq!(v[1].y(), 1141293.7960220212f64);
    /// ```
    ///
    /// # Safety
    /// This method contains unsafe code.
    // TODO: there may be a way of avoiding some allocations, but transmute won't work because
    // PJ_COORD and Point<T> are different sizes
    pub fn convert_array<'a, T>(
        &self,
        points: &'a mut [Point<T>],
    ) -> Result<&'a mut [Point<T>], ProjError>
    where
        T: Float,
    {
        self.array_general(points, Transformation::Conversion, false)
    }

    /// Project an array of geodetic coordinates (in radians) into the projection specified by `definition`
    ///
    /// **Note:** specifying `inverse` as `true` carries out an inverse projection *to* geodetic coordinates
    /// (in radians) from the projection specified by `definition`.
    ///
    /// ```rust
    /// use proj::Proj;
    /// extern crate geo_types;
    /// use geo_types::Point;
    /// # use assert_approx_eq::assert_approx_eq;
    /// let stereo70 = Proj::new(
    ///     "+proj=sterea +lat_0=46 +lon_0=25 +k=0.99975 +x_0=500000 +y_0=500000
    ///     +ellps=krass +towgs84=33.4,-146.6,-76.3,-0.359,-0.053,0.844,-0.84 +units=m +no_defs"
    /// )
    /// .unwrap();
    /// // Geodetic -> Pulkovo 1942(58) / Stereo70 (EPSG 3844)
    /// let mut v = vec![Point::new(0.436332, 0.802851)];
    /// let t = stereo70.project_array(&mut v, false).unwrap();
    /// assert_approx_eq!(v[0].x(), 500119.7035366755f64);
    /// assert_approx_eq!(v[0].y(), 500027.77901023754f64);
    /// ```
    ///
    /// # Safety
    /// This method contains unsafe code.
    // TODO: there may be a way of avoiding some allocations, but transmute won't work because
    // PJ_COORD and Point<T> are different sizes
    pub fn project_array<'a, T>(
        &self,
        points: &'a mut [Point<T>],
        inverse: bool,
    ) -> Result<&'a mut [Point<T>], ProjError>
    where
        T: Float,
    {
        self.array_general(points, Transformation::Projection, inverse)
    }

    // array conversion and projection logic is almost identical;
    // transform points in input array into PJ_COORD, transform them, error-check, then re-fill
    // input slice with points. Only the actual transformation ops vary slightly.
    fn array_general<'a, T>(
        &self,
        points: &'a mut [Point<T>],
        op: Transformation,
        inverse: bool,
    ) -> Result<&'a mut [Point<T>], ProjError>
    where
        T: Float,
    {
        let err;
        let trans;
        let inv = if inverse {
            PJ_DIRECTION_PJ_INV
        } else {
            PJ_DIRECTION_PJ_FWD
        };
        // we need PJ_COORD to convert
        let mut pj = points
            .iter()
            .map(|point| {
                let c_x: c_double = point.x().to_f64().ok_or(ProjError::FloatConversion)?;
                let c_y: c_double = point.y().to_f64().ok_or(ProjError::FloatConversion)?;
                Ok(PJ_COORD {
                    xy: PJ_XY { x: c_x, y: c_y },
                })
            })
            .collect::<Result<Vec<_>, ProjError>>()?;
        pj.shrink_to_fit();
        // Transformation operations are slightly different
        match op {
            Transformation::Conversion => unsafe {
                proj_errno_reset(self.c_proj);
                trans =
                    proj_trans_array(self.c_proj, PJ_DIRECTION_PJ_FWD, pj.len(), pj.as_mut_ptr());
                err = proj_errno(self.c_proj);
            },
            Transformation::Projection => unsafe {
                proj_errno_reset(self.c_proj);
                trans = proj_trans_array(self.c_proj, inv, pj.len(), pj.as_mut_ptr());
                err = proj_errno(self.c_proj);
            },
        }
        if err == 0 && trans == 0 {
            // re-fill original slice with Points
            // feels a bit clunky, but we're guaranteed that pj and points have the same length
            unsafe {
                for (i, coord) in pj.iter().enumerate() {
                    points[i] = Point::new(
                        T::from(coord.xy.x).ok_or(ProjError::FloatConversion)?,
                        T::from(coord.xy.y).ok_or(ProjError::FloatConversion)?,
                    )
                }
            }
            Ok(points)
        } else {
            Err(ProjError::Projection(error_message(err)?))
        }
    }
}

impl Drop for Proj {
    fn drop(&mut self) {
        unsafe {
            if let Some(area) = self.area {
                proj_area_destroy(area)
            }
            proj_destroy(self.c_proj);
            proj_context_destroy(self.ctx);
            // NB do NOT call until proj_destroy and proj_context_destroy have both returned:
            // https://proj.org/development/reference/functions.html#c.proj_cleanup
            proj_cleanup()
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use geo_types::Point;

    fn assert_almost_eq(a: f64, b: f64) {
        let f: f64 = a / b;
        assert!(f < 1.00001);
        assert!(f > 0.99999);
    }
    #[test]
    fn test_definition() {
        let wgs84 = "+proj=longlat +datum=WGS84 +no_defs";
        let proj = Proj::new(wgs84).unwrap();
        assert_eq!(
            proj.def().unwrap(),
            "proj=longlat datum=WGS84 no_defs ellps=WGS84 towgs84=0,0,0"
        );
    }
    #[test]
    fn test_searchpath() {
        let wgs84 = "+proj=longlat +datum=WGS84 +no_defs";
        let proj = Proj::new(wgs84).unwrap();
        proj.set_search_paths(&"/foo").unwrap();
        let ipath = proj.info().unwrap().searchpath;
        let pathsep = if cfg!(windows) { ";" } else { ":" };
        let individual: Vec<&str> = ipath.split(pathsep).collect();
        assert_eq!(&individual.last().unwrap(), &&"/foo")
    }
    #[test]
    fn test_endpoint() {
        let ep = get_url_endpoint().unwrap();
        assert_eq!(&ep, "https://cdn.proj.org");
        set_url_endpoint("https://github.com/georust").unwrap();
        let ep = get_url_endpoint().unwrap();
        assert_eq!(&ep, "https://github.com/georust");
    }
    #[test]
    fn test_from_crs() {
        let from = "EPSG:2230";
        let to = "EPSG:26946";
        let proj = Proj::new_known_crs(&from, &to, None).unwrap();
        let t = proj
            .convert(Point::new(4760096.421921, 3744293.729449))
            .unwrap();
        assert_almost_eq(t.x(), 1450880.29);
        assert_almost_eq(t.y(), 1141263.01);
    }
    // This test is disabled by default as it requires network access
    #[test]
    fn test_network() {
        let from = "EPSG:4326";
        let to = "EPSG:4326+3855";
        // off by default
        assert_eq!(network_enabled(), false);
        // switch it on and disable cache for subsequent calls
        grid_cache_set_enable(false);
        enable_network(true).unwrap();
        let proj = Proj::new_known_crs(&from, &to, None).unwrap();
        assert_eq!(network_enabled(), true);
        let t = proj.convert(Point::new(40.0, -80.0)).unwrap();
        assert_almost_eq(t.x(), 39.99999839);
        assert_almost_eq(t.y(), -79.99999807);
    }
    #[test]
    // Carry out a projection from geodetic coordinates
    fn test_projection() {
        let stereo70 = Proj::new(
            "+proj=sterea +lat_0=46 +lon_0=25 +k=0.99975 +x_0=500000 +y_0=500000
            +ellps=krass +towgs84=33.4,-146.6,-76.3,-0.359,-0.053,0.844,-0.84 +units=m +no_defs",
        )
        .unwrap();
        // Geodetic -> Pulkovo 1942(58) / Stereo70 (EPSG 3844)
        let t = stereo70
            .project(Point::new(0.436332, 0.802851), false)
            .unwrap();
        assert_almost_eq(t.x(), 500119.7035366755);
        assert_almost_eq(t.y(), 500027.77901023754);
    }
    #[test]
    // Carry out an inverse projection to geodetic coordinates
    fn test_inverse_projection() {
        let stereo70 = Proj::new(
            "+proj=sterea +lat_0=46 +lon_0=25 +k=0.99975 +x_0=500000 +y_0=500000
            +ellps=krass +towgs84=33.4,-146.6,-76.3,-0.359,-0.053,0.844,-0.84 +units=m +no_defs",
        )
        .unwrap();
        // Pulkovo 1942(58) / Stereo70 (EPSG 3844) -> Geodetic
        let t = stereo70
            .project(Point::new(500119.70352012233, 500027.77896348457), true)
            .unwrap();
        assert_almost_eq(t.x(), 0.436332);
        assert_almost_eq(t.y(), 0.802851);
    }
    #[test]
    // Carry out an inverse projection to geodetic coordinates
    fn test_london_inverse() {
        let osgb36 = Proj::new(
            "
            +proj=tmerc +lat_0=49 +lon_0=-2 +k=0.9996012717 +x_0=400000 +y_0=-100000 +ellps=airy
            +towgs84=446.448,-125.157,542.06,0.15,0.247,0.842,-20.489 +units=m +no_defs
            ",
        )
        .unwrap();
        // OSGB36 (EPSG 27700) -> Geodetic
        let t = osgb36
            .project(Point::new(548295.39, 182498.46), true)
            .unwrap();
        assert_almost_eq(t.x(), 0.0023755864848281206);
        assert_almost_eq(t.y(), 0.8992274896304518);
    }
    #[test]
    // Carry out a conversion from NAD83 feet (EPSG 2230) to NAD83 metres (EPSG 26946)
    fn test_conversion() {
        let nad83_m = Proj::new("
            +proj=pipeline
            +step +inv +proj=lcc +lat_1=33.88333333333333
            +lat_2=32.78333333333333 +lat_0=32.16666666666666
            +lon_0=-116.25 +x_0=2000000.0001016 +y_0=500000.0001016001 +ellps=GRS80
            +towgs84=0,0,0,0,0,0,0 +units=us-ft +no_defs
            +step +proj=lcc +lat_1=33.88333333333333 +lat_2=32.78333333333333 +lat_0=32.16666666666666
            +lon_0=-116.25 +x_0=2000000 +y_0=500000
            +ellps=GRS80 +towgs84=0,0,0,0,0,0,0 +units=m +no_defs
        ").unwrap();
        // Presidio, San Francisco
        let t = nad83_m
            .convert(Point::new(4760096.421921, 3744293.729449))
            .unwrap();
        assert_almost_eq(t.x(), 1450880.29);
        assert_almost_eq(t.y(), 1141263.01);
    }
    #[test]
    // Test that instantiation fails wth bad proj string input
    fn test_init_error() {
        assert!(Proj::new("🦀").is_none());
    }
    #[test]
    fn test_conversion_error() {
        // because step 1 isn't an inverse conversion, it's expecting lon lat input
        let nad83_m = Proj::new(
            "+proj=geos +lon_0=0.00 +lat_0=0.00 +a=6378169.00 +b=6356583.80 +h=35785831.0",
        )
        .unwrap();
        let err = nad83_m
            .convert(Point::new(4760096.421921, 3744293.729449))
            .unwrap_err();
        assert_eq!(
            "The conversion failed with the following error: latitude or longitude exceeded limits",
            err.to_string()
        );
    }

    #[test]
    fn test_error_recovery() {
        let nad83_m = Proj::new(
            "+proj=geos +lon_0=0.00 +lat_0=0.00 +a=6378169.00 +b=6356583.80 +h=35785831.0",
        )
        .unwrap();

        // we expect this first conversion to fail (copied from above test case)
        assert!(nad83_m
            .convert(Point::new(4760096.421921, 3744293.729449))
            .is_err());

        // but a subsequent valid conversion should still be successful
        assert!(nad83_m.convert(Point::new(0.0, 0.0)).is_ok());

        // also test with project() function
        assert!(nad83_m
            .project(Point::new(99999.0, 99999.0), false)
            .is_err());
        assert!(nad83_m.project(Point::new(0.0, 0.0), false).is_ok());
    }

    #[test]
    fn test_array_convert() {
        let from = "EPSG:2230";
        let to = "EPSG:26946";
        let ft_to_m = Proj::new_known_crs(&from, &to, None).unwrap();
        let mut v = vec![
            Point::new(4760096.421921, 3744293.729449),
            Point::new(4760197.421921, 3744394.729449),
        ];
        ft_to_m.convert_array(&mut v).unwrap();
        assert_almost_eq(v[0].x(), 1450880.2910605003f64);
        assert_almost_eq(v[1].y(), 1141293.7960220212f64);
    }

    #[test]
    // Ensure that input and output order are normalised to Lon, Lat / Easting Northing
    // Without normalisation this test would fail, as EPSG:4326 expects Lat, Lon input order.
    fn test_input_order() {
        let from = "EPSG:4326";
        let to = "EPSG:2230";
        let to_feet = Proj::new_known_crs(&from, &to, None).unwrap();
        // 👽
        let usa_m = Point::new(-115.797615, 37.2647978);
        let usa_ft = to_feet.convert(usa_m).unwrap();
        assert_eq!(6693625.67217475, usa_ft.x());
        assert_eq!(3497301.5918027186, usa_ft.y());
    }
}
