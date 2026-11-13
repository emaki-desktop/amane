use vello::kurbo::Affine;
use vello::peniko::{self, ImageData};
use vello::wgpu::Texture;
use vello::{AaConfig, RenderParams, Scene};

use super::bounds::Place;
use super::{Gpu, texture};

impl Gpu {
    // vello always clears what it draws into, so it draws apart and the result is laid on top
    pub(super) fn paint(
        &mut self,
        scene: &mut Scene,
        canvas: &Texture,
        borrowed: &mut Vec<ImageData>,
    ) {
        let reach = std::mem::take(&mut self.reach);

        if scene.encoding().is_empty() {
            return;
        }

        // the same place in the frame usually gets the same scene as last frame
        let index = self.painting;

        self.painting += 1;

        let (width, height) = (canvas.width(), canvas.height());

        // a run drawn wholly off the canvas shows nothing
        let Some(place) = reach.place(width, height) else {
            scene.reset();

            return;
        };

        if let Some(kept) = self.unchanged(index, scene, place) {
            self.lay_at(kept, canvas, place);
        } else {
            let scratch = if place.is_whole(width, height) {
                self.render(scene, width, height)
            } else {
                // moved so the part's corner lands on the small texture's corner
                let mut moved = std::mem::take(&mut self.moved);

                moved.reset();
                moved.append(
                    scene,
                    Some(Affine::translate((
                        -f64::from(place.x),
                        -f64::from(place.y),
                    ))),
                );

                let (width, height) = place.texture(width, height);

                let scratch = self.render(&moved, width, height);

                self.moved = moved;

                scratch
            };

            self.lay_at(&scratch, canvas, place);

            self.keep(index, scene, place, scratch);
        }

        scene.reset();

        for image in borrowed.drain(..) {
            self.vello.borrow_mut().unregister_texture(image);
        }
    }

    // the scene alone, on its own texture of the given size, given back by the caller
    pub(super) fn render(&mut self, scene: &Scene, width: u32, height: u32) -> Texture {
        self.runs += 1;

        self.vello_used.set(std::time::Instant::now());
        self.vello_released.set(false);
        super::shared::vello_ran();

        self.keep_atlas(scene);

        let scratch = self.take_scratch(width, height);

        self.submit();

        let params = RenderParams {
            base_color: peniko::Color::TRANSPARENT,
            width,
            height,
            antialiasing_method: AaConfig::Area,
        };

        #[cfg(feature = "vello-stats")]
        if crate::timing::enabled() {
            self.render_counted(scene, &scratch, &params);

            return scratch;
        }

        self.vello
            .borrow_mut()
            .render_to_texture(
                &self.device,
                &self.queue,
                scene,
                &texture::view(&scratch),
                &params,
            )
            .expect("failed to draw scene");

        scratch
    }

    /*
     * the scene drawn the slow way, reading back what it took of vello's
     * scratch buffers, to check a scene against the fork's shell-sized limits;
     * `failed 1` means a buffer overflowed and the run came out blank
     */
    #[cfg(feature = "vello-stats")]
    fn render_counted(&mut self, scene: &Scene, scratch: &Texture, params: &RenderParams) {
        use std::pin::pin;
        use std::task::{Context, Poll, Waker};

        let view = texture::view(scratch);
        let mut vello = self.vello.borrow_mut();

        #[expect(deprecated, reason = "the only way to read the bump counters back")]
        let mut render = pin!(vello.render_to_texture_async(
            &self.device,
            &self.queue,
            scene,
            &view,
            params,
            vello::low_level::DebugLayers::none(),
        ));

        // the readback's future waits on a mapping that only completes when the device is polled
        let mut context = Context::from_waker(Waker::noop());

        let counted = loop {
            match render.as_mut().poll(&mut context) {
                Poll::Ready(result) => break result.expect("failed to draw scene"),
                Poll::Pending => {
                    self.device
                        .poll(vello::wgpu::PollType::wait_indefinitely())
                        .expect("failed to wait for the gpu");
                }
            }
        };

        let Some(used) = counted else {
            return;
        };

        eprintln!(
            "scratch {}x{} failed {} tiles {} lines {} seg_counts {} segments {} ptcl {} bin {} blend {}",
            params.width,
            params.height,
            used.failed,
            used.tile,
            used.lines,
            used.seg_counts,
            used.segments,
            used.ptcl,
            used.binning,
            used.blend,
        );
    }

    // vello's result laid only over its part of the canvas
    fn lay_at(&self, source: &Texture, canvas: &Texture, place: Place) {
        let mut encoder = self.take_encoder();

        self.composite.run_within(
            &self.device,
            &mut encoder,
            &texture::view(source),
            &texture::view(canvas),
            [1.0, 1.0, place.x as f32, place.y as f32],
            Some(place),
        );

        self.keep_encoder(encoder);
    }

    pub(super) fn lay(&self, source: &Texture, canvas: &Texture, opacity: f32, premultiply: bool) {
        let premultiply = if premultiply { 1.0 } else { 0.0 };

        let mut encoder = self.take_encoder();

        self.composite.run(
            &self.device,
            &mut encoder,
            &texture::view(source),
            &texture::view(canvas),
            [opacity, premultiply, 0.0, 0.0],
        );

        self.keep_encoder(encoder);
    }
}
