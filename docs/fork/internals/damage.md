# Buffer damage (design, not built)

A changed frame today re-renders and presents its whole surface, and the compositor is told the
whole buffer changed. On niri that makes every changed bar frame a full-surface composite, and on
an OLED panel with Panel Replay it sends the whole strip over the eDP link instead of the few
pixels that changed (a clock digit, a battery percentage). This page says why amane can't tell the
compositor less yet, and what it would take. Written 2026-11-13, for the energy plan's item C10.

## Why not now

amane presents through wgpu's `Surface` (`graphics/gpu/present.rs`, `SurfaceTexture::present`).
The driver's window-system code then attaches the buffer, damages it and commits, all inside the
present call:

- **Vulkan (Mesa WSI):** `vkQueuePresentKHR` damages the whole buffer
  (`wl_surface.damage_buffer(0, 0, INT32_MAX, INT32_MAX)`) unless the present carries
  `VK_KHR_incremental_present` regions. wgpu doesn't expose present regions.
- **GL (EGL):** `eglSwapBuffers` damages everything; `eglSwapBuffersWithDamageKHR` takes rects,
  but wgpu calls the plain one.

Damage amane sends on the `wl_surface` before presenting only joins the full-buffer damage the
driver adds in the same commit, so it changes nothing. Presenting while holding back the commit
isn't possible either: the driver commits.

## What it would take

1. **Dirty rectangles from the command list.** `Gpu::shows` already compares the frame's
   commands with the ones on screen (`graphics/gpu/present.rs`). Comparing them command by
   command instead, and keeping each command's bounds, gives the rectangles that changed: the
   union of the old and new bounds of every command that differs, grown by blur and shadow reach
   and rounded out to whole buffer pixels at the window's scale. Group textures and vibrant layers
   (`wayland/vibrancy.rs`) each need their own.
2. **A present that takes damage.** One of:
   - a wgpu change exposing `VK_KHR_incremental_present` regions on `SurfaceTexture::present`
     (worth sending upstream: other Wayland clients want it too);
   - amane presenting itself for Wayland: render into a `wl_buffer` it owns (dmabuf from a wgpu
     texture with an exportable allocation, as `allocator.rs` already does for some buffers),
     then `attach` + `damage_buffer` per rectangle + `commit` on its own `wl_surface`. That needs
     a pool of 2–3 buffers and `wl_buffer.release` tracking, as a swapchain does.
3. **Partial rendering.** The real GPU saving needs the renderer to redraw only inside the damage:
   a scissor per rectangle on the quads pipeline, and the previous frame's buffer content kept
   (buffer age), so a buffer that's two frames old gets the union of both frames' damage.

Steps 1 and 2 alone already help the compositor and the panel link (niri composites only the
damage, and Panel Replay's selective update sends only those lines). Step 3 also saves amane's
own GPU time.

## Until then

What keeps the cost down now:

- an identical frame isn't drawn, and isn't committed unless something was sent with it
  (`wayland/redraw.rs`);
- frozen windows (`LayerWindow::frozen`) draw nothing, which the shell uses for its hidden bar,
  dock and covered widgets;
- `set_screen_off` stops drawing while the monitors are off.
