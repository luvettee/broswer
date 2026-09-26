//! Site icons: fetched once per site, shrunk to a 32×32 bitmap (about 4 KB),
//! and kept on disk so restored tabs show their icon before they load.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use block2::RcBlock;
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSCompositingOperation, NSDeviceRGBColorSpace,
    NSGraphicsContext, NSImage, NSImageInterpolation,
};
use objc2_foundation::{
    MainThreadMarker, NSData, NSDictionary, NSError, NSHTTPURLResponse, NSRect, NSSize, NSString,
    NSURL, NSURLResponse, NSURLSession, NSURLSessionConfiguration,
};

use crate::log;
use crate::msg::{Msg, MsgSender};

const PIXELS: isize = 32;
const MAX_BYTES: usize = 512 * 1024;
/// Icons held in memory; each is a few kilobytes.
const MEMORY_LIMIT: usize = 256;

fn dir() -> PathBuf {
    crate::blocker::data_dir().join("icons")
}

fn file(site: &str) -> PathBuf {
    let name: String = site
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    dir().join(format!("{name}.png"))
}

/// Reuse the existing on-disk favicon without making requests from the start page.
pub fn cached_png(site: &str) -> Option<Vec<u8>> {
    std::fs::read(file(site))
        .ok()
        .filter(|bytes| bytes.len() <= 16 * 1024)
}

/// A session without a response cache: icons are cached on disk by us, so
/// keeping another copy in memory would only cost RAM.
fn session() -> Retained<NSURLSession> {
    let config = NSURLSessionConfiguration::ephemeralSessionConfiguration();
    config.setURLCache(None);
    config.setHTTPMaximumConnectionsPerHost(2);
    NSURLSession::sessionWithConfiguration(&config)
}

/// Downloads an icon off the main thread; the result arrives as `Msg::IconLoaded`.
fn fetch(session: &NSURLSession, icon_url: &str, site: String, tx: MsgSender) {
    let Some(url) = NSURL::URLWithString(&NSString::from_str(icon_url)) else {
        let _ = tx.send(Msg::IconLoaded(site, None));
        return;
    };
    let done = RcBlock::new(
        move |data: *mut NSData, response: *mut NSURLResponse, _error: *mut NSError| {
            let ok = unsafe { response.as_ref() }
                .and_then(|r| r.downcast_ref::<NSHTTPURLResponse>())
                .is_some_and(|r| (200..300).contains(&r.statusCode()));
            let bytes = unsafe { data.as_ref() }
                .filter(|_| ok)
                .filter(|d| d.len() > 0 && d.len() <= MAX_BYTES)
                .map(|d| d.to_vec());
            let _ = tx.send(Msg::IconLoaded(site.clone(), bytes));
        },
    );
    let task = unsafe { session.dataTaskWithURL_completionHandler(&url, &done) };
    task.resume();
}

/// Draws any image into a small bitmap so large icons don't stay decoded in memory.
fn rasterize(source: &NSImage) -> Option<Retained<NSBitmapImageRep>> {
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            PIXELS,
            PIXELS,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            0,
            0,
        )
    }?;
    let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&context));
    context.setImageInterpolation(NSImageInterpolation::High);
    let size = PIXELS as f64;
    source.drawInRect_fromRect_operation_fraction(
        NSRect::new(
            objc2_foundation::NSPoint::new(0.0, 0.0),
            NSSize::new(size, size),
        ),
        NSRect::ZERO,
        NSCompositingOperation::SourceOver,
        1.0,
    );
    context.flushGraphics();
    NSGraphicsContext::restoreGraphicsState_class();
    Some(rep)
}

fn image_from(rep: &NSBitmapImageRep, mtm: MainThreadMarker) -> Retained<NSImage> {
    let _ = mtm;
    let image = NSImage::initWithSize(NSImage::alloc(), NSSize::new(16.0, 16.0));
    image.addRepresentation(rep);
    image
}

#[derive(Default)]
pub struct Icons {
    images: HashMap<String, Retained<NSImage>>,
    /// Sites with no icon file on disk, so lookups skip the file system.
    missing: HashSet<String>,
    /// Sites being downloaded, or whose download failed this session.
    tried: HashSet<String>,
    session: Option<Retained<NSURLSession>>,
}

impl Icons {
    /// The icon for `site`, from memory or the disk cache.
    pub fn get(&mut self, site: &str) -> Option<Retained<NSImage>> {
        if let Some(image) = self.images.get(site) {
            return Some(image.clone());
        }
        if self.missing.contains(site) {
            return None;
        }
        let path = file(site);
        let image = MainThreadMarker::new().and_then(|_| {
            NSImage::initWithContentsOfFile(
                NSImage::alloc(),
                &NSString::from_str(&path.to_string_lossy()),
            )
        });
        let Some(image) = image else {
            self.missing.insert(site.to_string());
            return None;
        };
        image.setSize(NSSize::new(16.0, 16.0));
        self.remember(site, image.clone());
        Some(image)
    }

    /// Forgets decoded icons; views showing one keep their own reference.
    pub fn trim(&mut self) {
        self.images = HashMap::new();
        self.missing = HashSet::new();
    }

    fn remember(&mut self, site: &str, image: Retained<NSImage>) {
        if self.images.len() >= MEMORY_LIMIT {
            self.images.clear();
        }
        self.images.insert(site.to_string(), image);
    }

    /// A page announced its icon. Fetches it unless the cached one is recent.
    pub fn found(&mut self, site: &str, icon_url: &str, tx: &MsgSender) {
        if !(icon_url.starts_with("https://") || icon_url.starts_with("http://")) {
            return;
        }
        let fresh_on_disk = std::fs::metadata(file(site))
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age.as_secs() < 7 * 86_400);
        if fresh_on_disk || !self.tried.insert(site.to_string()) {
            return;
        }
        let session = self.session.get_or_insert_with(session);
        fetch(session, icon_url, site.to_string(), tx.clone());
    }

    /// A download finished. Returns true when a new icon is ready to show.
    pub fn loaded(&mut self, site: &str, bytes: Option<Vec<u8>>) -> bool {
        let Some(bytes) = bytes else {
            return false;
        };
        let Some(mtm) = MainThreadMarker::new() else {
            return false;
        };
        let data = NSData::from_vec(bytes);
        let Some(source) = NSImage::initWithData(NSImage::alloc(), &data) else {
            return false;
        };
        let Some(rep) = rasterize(&source) else {
            return false;
        };
        if let Some(png) = unsafe {
            rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
        } {
            let path = file(site);
            if let Err(e) =
                std::fs::create_dir_all(dir()).and_then(|_| std::fs::write(&path, png.to_vec()))
            {
                log::log(&format!("icon cache write failed: {e}"));
            }
        }
        self.tried.remove(site);
        self.missing.remove(site);
        self.remember(site, image_from(&rep, mtm));
        true
    }
}
