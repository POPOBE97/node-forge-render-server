use super::*;

const THUMBNAIL_LIMIT: usize = 96;
type ThumbnailResult = Result<([u32; 2], Vec<u8>, Orientation, [u32; 2]), String>;
struct Thumbnail {
    texture: Option<(egui::TextureHandle, Orientation, [u32; 2])>,
    used: u64,
}
#[derive(Default)]
pub(super) struct Thumbnails {
    cache: HashMap<PathBuf, Thumbnail>,
    job: Option<(PathBuf, mpsc::Receiver<ThumbnailResult>)>,
    tick: u64,
}
impl Thumbnails {
    pub fn invalidate(&mut self, path: &Path) {
        self.cache.remove(path);
    }
    pub fn is_loading(&self) -> bool {
        self.job.is_some()
    }
    pub fn poll(&mut self, ctx: &egui::Context) {
        let ready = self.job.as_ref().and_then(|(_, rx)| rx.try_recv().ok());
        if let Some(result) = ready {
            let (path, _) = self.job.take().unwrap();
            let texture = result
                .ok()
                .map(|(size, pixels, orientation, original_size)| {
                    (
                        ctx.load_texture(
                            "sys.camera.thumbnail",
                            egui::ColorImage::from_rgba_unmultiplied(
                                [size[0] as usize, size[1] as usize],
                                &pixels,
                            ),
                            egui::TextureOptions::LINEAR,
                        ),
                        orientation,
                        original_size,
                    )
                });
            if self.cache.len() >= THUMBNAIL_LIMIT {
                if let Some(oldest) = self
                    .cache
                    .iter()
                    .min_by_key(|(_, value)| value.used)
                    .map(|(key, _)| key.clone())
                {
                    self.cache.remove(&oldest);
                }
            }
            self.cache.insert(
                path,
                Thumbnail {
                    texture,
                    used: self.tick,
                },
            );
        }
    }
    pub fn get(
        &mut self,
        path: &std::path::Path,
    ) -> Option<(egui::TextureHandle, Orientation, [u32; 2])> {
        self.tick += 1;
        if let Some(item) = self.cache.get_mut(path) {
            item.used = self.tick;
            return item.texture.clone();
        }
        if self.job.is_none() {
            let path = path.to_owned();
            let input = path.clone();
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let result = decode_photo(&input)
                    .and_then(|(size, bytes, orientation)| {
                        let original = image::RgbaImage::from_raw(size[0], size[1], bytes)
                            .ok_or_else(|| anyhow::anyhow!("Invalid image size"))?;
                        let image = image::DynamicImage::ImageRgba8(original)
                            .thumbnail(240, 144)
                            .into_rgba8();
                        Ok((
                            [image.width(), image.height()],
                            image.into_raw(),
                            orientation,
                            size,
                        ))
                    })
                    .map_err(|e| e.to_string());
                let _ = tx.send(result);
            });
            self.job = Some((path, rx));
        }
        None
    }
}

// Only one original decode runs at a time. A newer request replaces the pending request;
// completed work is displayed only if it still matches the selection generation.
#[derive(Default)]
pub(super) struct PhotoLoader {
    pub requested: Option<(u64, PathBuf)>,
    generation: u64,
    job: Option<(u64, mpsc::Receiver<PhotoResult>)>,
}
impl PhotoLoader {
    pub fn request(&mut self, path: PathBuf) {
        self.generation += 1;
        self.requested = Some((self.generation, path));
        self.start();
    }
    pub fn cancel(&mut self) {
        self.generation += 1;
        self.requested = None;
    }
    fn start(&mut self) {
        if self.job.is_some() {
            return;
        }
        if let Some((generation, path)) = self.requested.clone() {
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let result = decode_photo(&path)
                    .map(|(size, bytes, orientation)| (path, size, bytes, orientation))
                    .map_err(|e| e.to_string());
                let _ = tx.send(result);
            });
            self.job = Some((generation, rx));
        }
    }
    pub fn poll(&mut self) -> Option<PhotoResult> {
        let result = self
            .job
            .as_ref()
            .and_then(|(generation, rx)| rx.try_recv().ok().map(|result| (*generation, result)));
        if let Some((generation, result)) = result {
            self.job = None;
            if self
                .requested
                .as_ref()
                .is_some_and(|(wanted, _)| *wanted == generation)
            {
                self.requested = None;
                return Some(result);
            }
            self.start();
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_decode_and_cancelled_review_are_not_published() {
        let (tx, rx) = mpsc::channel();
        let mut loader = PhotoLoader {
            requested: Some((2, PathBuf::from("latest.jpg"))),
            generation: 2,
            job: Some((1, rx)),
        };
        tx.send(Ok((
            "old.jpg".into(),
            [1, 1],
            vec![0; 4],
            Orientation::NoTransforms,
        )))
        .unwrap();
        assert!(loader.poll().is_none());
        assert_eq!(loader.job.as_ref().map(|j| j.0), Some(2));
        loader.cancel();
        let (tx, rx) = mpsc::channel();
        loader.job = Some((2, rx));
        tx.send(Ok((
            "latest.jpg".into(),
            [1, 1],
            vec![0; 4],
            Orientation::NoTransforms,
        )))
        .unwrap();
        assert!(loader.poll().is_none());
        assert!(loader.requested.is_none());
    }
}
