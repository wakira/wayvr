use core::f32;
use std::sync::Arc;

use glam::Vec4;
use ovr_overlay::{
    overlay::{OverlayHandle, OverlayManager},
    pose::Matrix3x4,
    sys::{ETrackingUniverseOrigin, VROverlayFlags, VRVulkanTextureData_t},
};
use smallvec::SmallVec;
use wgui::gfx::{CommandBufferUsage, ImageLayout, ImageUsage, ImageView, WGfx};
use wlx_common::overlays::StereoMode;

use crate::{state::AppState, windowing::window::OverlayWindowData};

use super::helpers::Affine3AConvert;

#[derive(Default)]
pub(super) struct OpenVrOverlayData {
    pub(super) handle: Option<OverlayHandle>,
    pub(super) visible: bool,
    pub(super) color: Vec4,
    pub(crate) width: f32,
    pub(super) override_width: bool,
    /// Per-eye staging images. One element normally, two when stereo is active.
    pub(super) image_views: SmallVec<[Arc<ImageView>; 2]>,
    /// Combined side-by-side image submitted to the compositor when stereo is active.
    pub(super) combined_view: Option<Arc<ImageView>>,
    /// Stereo mode currently applied to this overlay.
    pub(super) stereo: StereoMode,
    pub(super) image_dirty: bool,
}

impl OverlayWindowData<OpenVrOverlayData> {
    pub(super) fn initialize(
        &mut self,
        overlay: &mut OverlayManager,
        app: &mut AppState,
    ) -> anyhow::Result<OverlayHandle> {
        let key = format!("wlx-{}", self.config.name);
        log::debug!("Create overlay with key: {key}");
        let handle = match overlay.create_overlay(&key, &key) {
            Ok(handle) => handle,
            Err(e) => {
                panic!("Failed to create overlay: {e}");
            }
        };
        log::debug!("{}: initialize", self.config.name);

        self.data.handle = Some(handle);
        self.data.color = Vec4::ONE;

        self.init(app)?;

        if self.data.width < f32::EPSILON {
            self.data.width = 1.0;
        }

        self.upload_width(overlay);
        self.upload_color(overlay);
        self.upload_alpha(overlay);
        self.upload_curvature(overlay);
        self.upload_sort_order(overlay);

        Ok(handle)
    }

    /// Ensures render-target images for the overlay exist at the requested extent.
    ///
    /// Returns one image normally, or two per-eye images when `stereo` is enabled.
    /// When stereo is enabled a combined side-by-side image is also (re)created,
    /// which is what eventually gets submitted to the compositor.
    pub(super) fn ensure_staging_images(
        &mut self,
        app: &mut AppState,
        extent: [u32; 2],
        stereo: StereoMode,
    ) -> anyhow::Result<SmallVec<[Arc<ImageView>; 2]>> {
        let eye_count = if matches!(stereo, StereoMode::None) {
            1
        } else {
            2
        };
        let combined_extent = [extent[0].saturating_mul(2), extent[1]];

        let needs_recreate = self.data.image_views.len() != eye_count
            || self.data.image_views.iter().any(|v| v.extent_2d() != extent)
            || (eye_count == 2
                && !self
                    .data
                    .combined_view
                    .as_ref()
                    .is_some_and(|v| v.extent_2d() == combined_extent));

        if needs_recreate {
            log::debug!(
                "{}: recreating staging images at {}x{} (eyes: {eye_count})",
                self.config.name,
                extent[0],
                extent[1],
            );

            let mut views: SmallVec<[Arc<ImageView>; 2]> = SmallVec::new();
            for _ in 0..eye_count {
                let image = app.gfx.new_image(
                    extent[0],
                    extent[1],
                    app.gfx.surface_format(),
                    ImageUsage::COLOR_ATTACHMENT | ImageUsage::TRANSFER_SRC | ImageUsage::SAMPLED,
                )?;
                views.push(app.gfx.create_image_view(image)?);
            }
            self.data.image_views = views;

            if eye_count == 2 {
                let combined = app.gfx.new_image(
                    combined_extent[0],
                    combined_extent[1],
                    app.gfx.surface_format(),
                    ImageUsage::TRANSFER_SRC | ImageUsage::TRANSFER_DST | ImageUsage::SAMPLED,
                )?;
                self.data.combined_view = Some(app.gfx.create_image_view(combined)?);
            } else {
                self.data.combined_view = None;
            }
        }

        Ok(self.data.image_views.clone())
    }

    /// Applies the OpenVR side-by-side flags matching the requested stereo mode.
    ///
    /// OpenVR overlays can only be split into a left/right pair, so top/bottom
    /// modes are packed into side-by-side in `upload_texture` and use the same flags.
    pub(super) fn upload_stereo(&mut self, overlay: &mut OverlayManager, stereo: StereoMode) {
        if self.data.stereo == stereo {
            return;
        }
        self.data.stereo = stereo;

        let Some(handle) = self.data.handle else {
            return;
        };

        let (parallel, crossed) = match stereo {
            StereoMode::LeftRight | StereoMode::TopBottom => (true, false),
            StereoMode::RightLeft | StereoMode::BottomTop => (false, true),
            StereoMode::None => (false, false),
        };

        if let Err(e) = overlay.set_overlay_flag(
            handle,
            VROverlayFlags::VROverlayFlags_SideBySide_Parallel,
            parallel,
        ) {
            log::error!(
                "{}: Failed to set side-by-side parallel flag: {}",
                self.config.name,
                e
            );
        }
        if let Err(e) = overlay.set_overlay_flag(
            handle,
            VROverlayFlags::VROverlayFlags_SideBySide_Crossed,
            crossed,
        ) {
            log::error!(
                "{}: Failed to set side-by-side crossed flag: {}",
                self.config.name,
                e
            );
        }
    }

    pub(super) fn after_input(
        &mut self,
        overlay: &mut OverlayManager,
        app: &mut AppState,
    ) -> anyhow::Result<()> {
        let want_visible = self
            .config
            .active_state
            .as_ref()
            .is_some_and(|x| x.alpha > 0.05);

        if want_visible && !self.data.visible {
            self.show_internal(overlay, app)?;
        } else if !want_visible && self.data.visible {
            self.hide_internal(overlay, app)?;
        }
        Ok(())
    }

    pub(super) fn after_render(
        &mut self,
        universe: ETrackingUniverseOrigin,
        overlay: &mut OverlayManager,
        graphics: &Arc<WGfx>,
    ) {
        if self.data.visible {
            if self.config.dirty {
                self.upload_curvature(overlay);

                self.upload_transform(universe, overlay);
                self.upload_alpha(overlay);
                self.config.dirty = false;
            }
            self.upload_texture(overlay, graphics);
        }
    }

    fn show_internal(
        &mut self,
        overlay: &mut OverlayManager,
        app: &mut AppState,
    ) -> anyhow::Result<()> {
        let handle = match self.data.handle {
            Some(handle) => handle,
            None => self.initialize(overlay, app)?,
        };
        log::debug!("{}: show", self.config.name);
        if let Err(e) = overlay.set_visibility(handle, true) {
            log::error!("{}: Failed to show overlay: {}", self.config.name, e);
        }
        self.data.visible = true;
        self.config.backend.resume(app)
    }

    fn hide_internal(
        &mut self,
        overlay: &mut OverlayManager,
        app: &mut AppState,
    ) -> anyhow::Result<()> {
        let Some(handle) = self.data.handle else {
            return Ok(());
        };
        log::debug!("{}: hide", self.config.name);
        if let Err(e) = overlay.set_visibility(handle, false) {
            log::error!("{}: Failed to hide overlay: {}", self.config.name, e);
        }
        self.data.visible = false;
        self.config.backend.pause(app)
    }

    pub(super) fn upload_alpha(&self, overlay: &mut OverlayManager) {
        let Some(handle) = self.data.handle else {
            log::debug!("{}: No overlay handle", self.config.name);
            return;
        };
        let Some(state) = self.config.active_state.as_ref() else {
            return;
        };
        if let Err(e) = overlay.set_opacity(handle, state.alpha) {
            log::error!("{}: Failed to set overlay alpha: {}", self.config.name, e);
        }
    }

    pub(super) fn upload_color(&self, overlay: &mut OverlayManager) {
        let Some(handle) = self.data.handle else {
            log::debug!("{}: No overlay handle", self.config.name);
            return;
        };
        if let Err(e) = overlay.set_tint(
            handle,
            ovr_overlay::ColorTint {
                r: self.data.color.x,
                g: self.data.color.y,
                b: self.data.color.z,
                a: self.data.color.w,
            },
        ) {
            log::error!("{}: Failed to set overlay tint: {}", self.config.name, e);
        }
    }

    fn upload_width(&self, overlay: &mut OverlayManager) {
        let Some(handle) = self.data.handle else {
            log::debug!("{}: No overlay handle", self.config.name);
            return;
        };
        if let Err(e) = overlay.set_width(handle, self.data.width) {
            log::error!("{}: Failed to set overlay width: {}", self.config.name, e);
        }
    }

    fn upload_curvature(&self, overlay: &mut OverlayManager) {
        let Some(handle) = self.data.handle else {
            log::debug!("{}: No overlay handle", self.config.name);
            return;
        };
        if let Err(e) = overlay.set_curvature(
            handle,
            self.config
                .active_state
                .as_ref()
                .unwrap()
                .curvature
                .unwrap_or(0.0),
        ) {
            log::error!(
                "{}: Failed to set overlay curvature: {}",
                self.config.name,
                e
            );
        }
    }

    fn upload_sort_order(&self, overlay: &mut OverlayManager) {
        let Some(handle) = self.data.handle else {
            log::debug!("{}: No overlay handle", self.config.name);
            return;
        };
        if let Err(e) = overlay.set_sort_order(handle, self.config.z_order) {
            log::error!("{}: Failed to set overlay z order: {}", self.config.name, e);
        }
    }

    pub(super) fn upload_transform(
        &mut self,
        universe: ETrackingUniverseOrigin,
        overlay: &mut OverlayManager,
    ) {
        let Some(handle) = self.data.handle else {
            log::debug!("{}: No overlay handle", self.config.name);
            return;
        };
        let Some(state) = self.config.active_state.as_ref() else {
            return;
        };

        let effective = state.transform
            * self
                .config
                .backend
                .frame_meta()
                .map(|f| f.transform)
                .unwrap_or_default();

        let transform = Matrix3x4::from_affine(&effective);

        if let Err(e) = overlay.set_transform_absolute(handle, universe, &transform) {
            log::error!(
                "{}: Failed to set overlay transform: {}",
                self.config.name,
                e
            );
        }
    }

    pub(super) fn upload_texture(&mut self, overlay: &mut OverlayManager, graphics: &Arc<WGfx>) {
        let Some(handle) = self.data.handle else {
            log::debug!("{}: No overlay handle", self.config.name);
            return;
        };

        if !self.data.image_dirty {
            return;
        }
        self.data.image_dirty = false;

        let stereo = !matches!(self.data.stereo, StereoMode::None);

        let image = if stereo {
            // Composite the two per-eye images into a single side-by-side texture.
            // The compositor's side-by-side flags then split it per eye.
            let (Some(left), Some(right), Some(combined)) = (
                self.data.image_views.first(),
                self.data.image_views.get(1),
                self.data.combined_view.as_ref(),
            ) else {
                log::warn!("{}: stereo staging images missing", self.config.name);
                return;
            };

            let left_image = left.image().clone();
            let right_image = right.image().clone();
            let combined_image = combined.image().clone();
            let eye_extent = left.extent_2d();

            let composite = || -> anyhow::Result<()> {
                let mut cmd =
                    graphics.create_xfer_command_buffer(CommandBufferUsage::OneTimeSubmit)?;
                let region = Some([eye_extent[0], eye_extent[1], 1]);
                cmd.copy_image(&left_image, [0, 0, 0], &combined_image, [0, 0, 0], region)?;
                cmd.copy_image(
                    &right_image,
                    [0, 0, 0],
                    &combined_image,
                    [eye_extent[0], 0, 0],
                    region,
                )?;
                cmd.build_and_execute_now()
            };

            if let Err(e) = composite() {
                log::error!(
                    "{}: Failed to composite stereo overlay image: {e:#}",
                    self.config.name
                );
                return;
            }

            combined_image
        } else {
            let Some(view) = self.data.image_views.first() else {
                log::debug!("{}: Not rendered", self.config.name);
                return;
            };
            view.image().clone()
        };

        let dimensions = image.extent();
        if !self.data.override_width {
            // For side-by-side stereo the compositor shows only half the texture
            // to each eye, so the perceived aspect ratio is half the texture's.
            let divisor = if stereo { 2.0 } else { 1.0 };
            let new_width = ((dimensions[0] as f32) / (divisor * dimensions[1] as f32)).min(1.0);
            if (new_width - self.data.width).abs() > f32::EPSILON {
                log::info!("{}: New width {}", self.config.name, new_width);
                self.data.width = new_width;
                self.upload_width(overlay);
            }
        }

        if let Err(e) = graphics.transition_image_now(&image, ImageLayout::TransferSrc) {
            log::error!(
                "{}: Failed to prepare overlay image: {e:#}",
                self.config.name
            );
            return;
        }

        let format = image.format();
        let mut texture = VRVulkanTextureData_t {
            m_nImage: image.raw_handle(),
            m_nFormat: graphics.raw_format(format) as _,
            m_nWidth: dimensions[0],
            m_nHeight: dimensions[1],
            m_nSampleCount: 1,
            m_pDevice: graphics.raw_device_handle() as *mut _,
            m_pPhysicalDevice: graphics.raw_physical_device_handle() as *mut _,
            m_pInstance: graphics.raw_instance_handle() as *mut _,
            m_pQueue: graphics.raw_graphics_queue_handle() as *mut _,
            m_nQueueFamilyIndex: graphics.graphics_queue_family_index(),
        };
        log::trace!(
            "{}: UploadTex {:?}, {}x{}, {:?}",
            self.config.name,
            format,
            texture.m_nWidth,
            texture.m_nHeight,
            image.usage()
        );
        if let Err(e) = overlay.set_image_vulkan(handle, &mut texture) {
            log::error!("{}: Failed to set overlay texture: {}", self.config.name, e);
        }
    }

    pub(super) fn destroy(&mut self, overlay: &mut OverlayManager) {
        if let Some(handle) = self.data.handle {
            log::debug!("{}: destroy", self.config.name);
            if let Err(e) = overlay.destroy_overlay(handle) {
                log::error!("{}: Failed to destroy overlay: {}", self.config.name, e);
            }
        }
    }
}
