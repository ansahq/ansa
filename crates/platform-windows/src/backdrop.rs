//! A compositor-only blur of the live scene behind the launcher.

use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::Rc;
use windows::Foundation::{IPropertyValue, PropertyValue};
use windows::Graphics::Effects::{
    IGraphicsEffect, IGraphicsEffectSource, IGraphicsEffectSource_Impl, IGraphicsEffect_Impl,
};
use windows::System::{DispatcherQueue, DispatcherQueueController};
use windows::Win32::Foundation::{E_INVALIDARG, HWND};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_HOSTBACKDROPBRUSH};
use windows::Win32::System::WinRT::Composition::ICompositorDesktopInterop;
use windows::Win32::System::WinRT::Graphics::Direct2D::{
    IGraphicsEffectD2D1Interop, IGraphicsEffectD2D1Interop_Impl, GRAPHICS_EFFECT_PROPERTY_MAPPING,
};
use windows::Win32::System::WinRT::{
    CreateDispatcherQueueController, DispatcherQueueOptions, DQTAT_COM_NONE, DQTYPE_THREAD_CURRENT,
};
use windows::UI::Composition::Desktop::DesktopWindowTarget;
use windows::UI::Composition::{
    CompositionEffectSourceParameter, CompositionRoundedRectangleGeometry, Compositor, SpriteVisual,
};
use windows::UI::ViewManagement::UISettings;
use windows_core::{implement, Interface, Result, BOOL, GUID, HSTRING, PCWSTR};
use windows_numerics::Vector2;

thread_local! {
    // If GPUI doesn't already have a queue, keep ours for the desktop UI
    // thread's lifetime. The WinRT compositor uses that same message pump.
    static QUEUE: RefCell<Option<DispatcherQueueController>> = const { RefCell::new(None) };
}

fn ensure_dispatcher_queue() -> Result<()> {
    if DispatcherQueue::GetForCurrentThread().is_ok() {
        return Ok(());
    }
    // SAFETY: called on GPUI's OLE-initialized UI thread, with its own pump.
    let queue = unsafe {
        CreateDispatcherQueueController(DispatcherQueueOptions {
            dwSize: std::mem::size_of::<DispatcherQueueOptions>() as u32,
            threadType: DQTYPE_THREAD_CURRENT,
            apartmentType: DQTAT_COM_NONE,
        })?
    };
    QUEUE.with(|slot| *slot.borrow_mut() = Some(queue));
    Ok(())
}

/// A Win2D-compatible effect description backed by Direct2D's built-in blur.
/// No application code can access or retain the backdrop pixels.
#[implement(IGraphicsEffect, IGraphicsEffectSource, IGraphicsEffectD2D1Interop)]
struct Frost {
    name: std::sync::RwLock<HSTRING>,
    source: IGraphicsEffectSource,
}

impl IGraphicsEffectSource_Impl for Frost_Impl {}

impl IGraphicsEffect_Impl for Frost_Impl {
    fn Name(&self) -> Result<HSTRING> {
        Ok(self.name.read().unwrap().clone())
    }

    fn SetName(&self, name: &HSTRING) -> Result<()> {
        *self.name.write().unwrap() = name.clone();
        Ok(())
    }
}

impl IGraphicsEffectD2D1Interop_Impl for Frost_Impl {
    fn GetEffectId(&self) -> Result<GUID> {
        // CLSID_D2D1GaussianBlur
        Ok(GUID::from_u128(0x1feb6d69_2fe6_4ac9_8c58_1d7f93e7a6a5))
    }

    fn GetNamedPropertyMapping(
        &self,
        _: &PCWSTR,
        _: *mut u32,
        _: *mut GRAPHICS_EFFECT_PROPERTY_MAPPING,
    ) -> Result<()> {
        // No animatable properties; the factory binds the fixed values below.
        Err(E_INVALIDARG.into())
    }

    fn GetPropertyCount(&self) -> Result<u32> {
        Ok(3)
    }

    fn GetProperty(&self, index: u32) -> Result<IPropertyValue> {
        match index {
            0 => PropertyValue::CreateSingle(24.0)?, // Standard deviation, in pixels.
            1 => PropertyValue::CreateUInt32(1)?,    // Balanced optimization.
            2 => PropertyValue::CreateUInt32(1)?,    // Hard border: don't fade to clear at edges.
            _ => return Err(E_INVALIDARG.into()),
        }
        .cast()
    }

    fn GetSource(&self, index: u32) -> Result<IGraphicsEffectSource> {
        match index {
            0 => Ok(self.source.clone()),
            _ => Err(E_INVALIDARG.into()),
        }
    }

    fn GetSourceCount(&self) -> Result<u32> {
        Ok(1)
    }
}

/// Native backdrop below GPUI's foreground swap chain. Own and drop it on the UI thread.
#[derive(Debug)]
pub struct LauncherBackdrop {
    sprite: SpriteVisual,
    clip: CompositionRoundedRectangleGeometry,
    _target: DesktopWindowTarget,
    _compositor: Compositor,
    _ui_thread: PhantomData<Rc<()>>,
}

impl LauncherBackdrop {
    /// Keep the blur inside GPUI's rounded panel, even when DWM declines to
    /// round a popup. The dimensions are in physical pixels.
    pub fn resize(&self, width: i32, height: i32, radius: i32) -> std::result::Result<(), String> {
        self.update_shape(width, height, radius)
            .map_err(|error| format!("Cannot size live backdrop: {error}"))
    }

    fn update_shape(&self, width: i32, height: i32, radius: i32) -> Result<()> {
        let size = Vector2 {
            X: width.max(0) as f32,
            Y: height.max(0) as f32,
        };
        let radius = radius.max(0) as f32;
        self.sprite.SetSize(size)?;
        self.clip.SetSize(size)?;
        self.clip.SetCornerRadius(Vector2 {
            X: radius,
            Y: radius,
        })
    }

    pub fn set_visible(&self, visible: bool) -> std::result::Result<(), String> {
        self.sprite
            .SetIsVisible(visible)
            .map_err(|error| format!("Cannot toggle live backdrop: {error}"))
    }

    /// Requires Windows 11 host-backdrop support and enabled transparency effects.
    pub fn new(
        hwnd: isize,
        width: i32,
        height: i32,
        radius: i32,
    ) -> std::result::Result<Self, String> {
        let window = super::visibility::composition_window(hwnd)?;
        let settings = UISettings::new().and_then(|settings| settings.AdvancedEffectsEnabled());
        match settings {
            Ok(true) => {}
            Ok(false) => return Err("Windows transparency effects are turned off".into()),
            Err(error) => {
                return Err(format!(
                    "Cannot read Windows transparency settings: {error}"
                ))
            }
        }
        let backdrop = Self::attach(HWND(window))
            .map_err(|error| format!("Cannot create live backdrop: {error}"))?;
        backdrop.resize(width, height, radius)?;
        Ok(backdrop)
    }

    fn attach(hwnd: HWND) -> Result<Self> {
        ensure_dispatcher_queue()?;
        let compositor = Compositor::new()?;
        let desktop: ICompositorDesktopInterop = compositor.cast()?;
        let parameter = HSTRING::from("Scene");
        let effect: IGraphicsEffect = Frost {
            name: Default::default(),
            source: CompositionEffectSourceParameter::Create(&parameter)?.cast()?,
        }
        .into();
        let blurred = compositor.CreateEffectFactory(&effect)?.CreateBrush()?;
        let sprite: SpriteVisual = compositor.CreateSpriteVisual()?;
        let clip = compositor.CreateRoundedRectangleGeometry()?;
        sprite.SetClip(&compositor.CreateGeometricClipWithGeometry(&clip)?)?;
        // SAFETY: this is the validated HWND of the calling UI thread. GPUI
        // owns the top target; the backdrop owns only the bottom target.
        let target = unsafe { desktop.CreateDesktopWindowTarget(hwnd, false)? };
        let enabled = BOOL(1);
        // SAFETY: same window; the BOOL lives for the duration of the call.
        unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_HOSTBACKDROPBRUSH,
                (&enabled as *const BOOL).cast(),
                std::mem::size_of::<BOOL>() as u32,
            )?
        };
        blurred.SetSourceParameter(&parameter, &compositor.CreateHostBackdropBrush()?)?;
        sprite.SetBrush(&blurred)?;
        target.SetRoot(&sprite)?;
        Ok(Self {
            sprite,
            clip,
            _target: target,
            _compositor: compositor,
            _ui_thread: PhantomData,
        })
    }
}
