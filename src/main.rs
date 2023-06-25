use gl::types::*;
use glutin::{
    config::{ConfigTemplateBuilder, GlConfig},
    context::{
        ContextApi, ContextAttributesBuilder, NotCurrentGlContextSurfaceAccessor,
        PossiblyCurrentContext,
    },
    display::{GetGlDisplay, GlDisplay},
    prelude::GlSurface,
    surface::{Surface as GlutinSurface, SurfaceAttributesBuilder, WindowSurface},
};
use glutin_winit::DisplayBuilder;
use raw_window_handle::HasRawWindowHandle;
use winit::{event_loop::EventLoopBuilder, event::{KeyboardInput, ElementState, VirtualKeyCode}};

use std::{ffi::CString, num::NonZeroU32};

use winit::{
    event::{Event, WindowEvent},
    event_loop::ControlFlow,
    window::{Window, WindowBuilder},
};

use skia_safe::{
    gpu::{gl::FramebufferInfo, BackendRenderTarget, SurfaceOrigin},
    ColorType, Surface, Color,
};

fn main() {
    let el = EventLoopBuilder::<ActionRequestEvent>::with_user_event().build();
    let winit_window_builder = WindowBuilder::new().with_title("rust-skia-gl-window").with_visible(false);

    let template = ConfigTemplateBuilder::new()
        .with_alpha_size(8)
        .with_transparency(true);

    let display_builder = DisplayBuilder::new().with_window_builder(Some(winit_window_builder));
    let (window, gl_config) = display_builder
        .build(&el, template, |configs| {
            configs
                .reduce(|accum, config| {
                    let transparency_check = config.supports_transparency().unwrap_or(false)
                        & !accum.supports_transparency().unwrap_or(false);

                    if transparency_check || config.num_samples() < accum.num_samples() {
                        config
                    } else {
                        accum
                    }
                })
                .unwrap()
        })
        .unwrap();

    let window = window.expect("Could not create window with OpenGL context");

    let state = State::new();

    let adapter = {
        let state = Arc::clone(&state);
        Adapter::new(
            &window,
            move || {
                let mut state = state.lock().unwrap();
                state.build_initial_tree()
            },
            el.create_proxy(),
        )
    };

    window.set_visible(true);

    let raw_window_handle = window.raw_window_handle();
    let context_attributes = ContextAttributesBuilder::new().build(Some(raw_window_handle));
    let fallback_context_attributes = ContextAttributesBuilder::new()
        .with_context_api(ContextApi::Gles(None))
        .build(Some(raw_window_handle));
    let not_current_gl_context = unsafe {
        gl_config
            .display()
            .create_context(&gl_config, &context_attributes)
            .unwrap_or_else(|_| {
                gl_config
                    .display()
                    .create_context(&gl_config, &fallback_context_attributes)
                    .expect("failed to create context")
            })
    };

    let attrs = SurfaceAttributesBuilder::<WindowSurface>::new().build(
        raw_window_handle,
        NonZeroU32::new(600).unwrap(),
        NonZeroU32::new(300).unwrap(),
    );

    let gl_surface = unsafe {
        gl_config
            .display()
            .create_window_surface(&gl_config, &attrs)
            .expect("Could not create gl window surface")
    };

    let gl_context = not_current_gl_context
        .make_current(&gl_surface)
        .expect("Could not make GL context current when setting up skia renderer");

    gl::load_with(|s| {
        gl_config
            .display()
            .get_proc_address(CString::new(s).unwrap().as_c_str())
    });
    let interface = skia_safe::gpu::gl::Interface::new_load_with(|name| {
        if name == "eglGetCurrentDisplay" {
            return std::ptr::null();
        }
        gl_config
            .display()
            .get_proc_address(CString::new(name).unwrap().as_c_str())
    })
    .expect("Could not create interface");

    let mut gr_context = skia_safe::gpu::DirectContext::new_gl(Some(interface), None)
        .expect("Could not create direct context");

    let fb_info = {
        let mut fboid: GLint = 0;
        unsafe { gl::GetIntegerv(gl::FRAMEBUFFER_BINDING, &mut fboid) };

        FramebufferInfo {
            fboid: fboid.try_into().unwrap(),
            format: skia_safe::gpu::gl::Format::RGBA8.into(),
        }
    };

    window.set_inner_size(winit::dpi::Size::new(winit::dpi::LogicalSize::new(
        600.0, 300.0,
    )));

    fn create_surface(
        fb_info: FramebufferInfo,
        gr_context: &mut skia_safe::gpu::DirectContext,
        num_samples: usize,
        stencil_size: usize,
    ) -> Surface {
        let size = (600, 300);
        let backend_render_target =
            BackendRenderTarget::new_gl(size, num_samples, stencil_size, fb_info);

        Surface::from_backend_render_target(
            gr_context,
            &backend_render_target,
            SurfaceOrigin::BottomLeft,
            ColorType::RGBA8888,
            None,
            None,
        )
        .expect("Could not create skia surface")
    }
    let num_samples = gl_config.num_samples() as usize;
    let stencil_size = gl_config.stencil_size() as usize;

    let surface = create_surface(fb_info, &mut gr_context, num_samples, stencil_size);

    struct Env {
        surface: Surface,
        gl_surface: GlutinSurface<WindowSurface>,
        gr_context: skia_safe::gpu::DirectContext,
        gl_context: PossiblyCurrentContext,
        #[allow(unused)]
        window: Window,
    }

    

    let mut env = Env {
        surface,
        gl_surface,
        gl_context,
        gr_context,
        window,
    };

    el.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::LoopDestroyed => {}
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => {
                    *control_flow = ControlFlow::Exit;
                }
                WindowEvent::Resized(physical_size) => {
                    env.surface =
                        create_surface(fb_info, &mut env.gr_context, num_samples, stencil_size);
                    /* First resize the opengl drawable */
                    let (width, height): (u32, u32) = physical_size.into();

                    env.gl_surface.resize(
                        &env.gl_context,
                        NonZeroU32::new(width.max(1)).unwrap(),
                        NonZeroU32::new(height.max(1)).unwrap(),
                    );
                }
                WindowEvent::Focused(is_window_focused) => {
                    let mut state = state.lock().unwrap();
                    state.is_window_focused = is_window_focused;
                    state.update_focus(&adapter);
                }
                WindowEvent::KeyboardInput {
                    input:
                        KeyboardInput {
                            virtual_keycode: Some(virtual_code),
                            state: ElementState::Pressed,
                            ..
                        },
                    ..
                } => match virtual_code {
                    VirtualKeyCode::Tab => {
                        let mut state = state.lock().unwrap();
                        state.focus = if state.focus == BUTTON_1_ID {
                            BUTTON_2_ID
                        } else {
                            BUTTON_1_ID
                        };
                        state.update_focus(&adapter);
                    }
                    VirtualKeyCode::Space => {
                        let mut state = state.lock().unwrap();
                        let id = state.focus;
                        state.press_button(&adapter, id);
                    }
                    _ => (),
                },
                _ => (),
            },
            Event::UserEvent(ActionRequestEvent {
                request:
                    ActionRequest {
                        action,
                        target,
                        data: None,
                    },
                ..
            }) if target == BUTTON_1_ID || target == BUTTON_2_ID => {
                let mut state = state.lock().unwrap();
                match action {
                    Action::Focus => {
                        state.focus = target;
                        state.update_focus(&adapter);
                    }
                    Action::Default => {
                        state.press_button(&adapter, target);
                    }
                    _ => (),
                }
            }
            Event::RedrawRequested(_) => {
                println!("DRAWING");
                let canvas = env.surface.canvas();
                canvas.clear(Color::BLUE);
                env.gr_context.flush_and_submit();
                env.gl_surface.swap_buffers(&env.gl_context).unwrap();
            }
            _ => (),
        }
    });
}

use std::{
    num::NonZeroU128,
    sync::{Arc, Mutex},
};
use accesskit::{
    Action, ActionRequest, DefaultActionVerb, Live, Node, NodeBuilder, NodeClassSet, NodeId, Rect,
    Role, Tree, TreeUpdate,
};
use accesskit_winit::{ActionRequestEvent, Adapter};

const WINDOW_TITLE: &str = "Hello world";

const WINDOW_ID: NodeId = NodeId(unsafe { NonZeroU128::new_unchecked(1) });
const BUTTON_1_ID: NodeId = NodeId(unsafe { NonZeroU128::new_unchecked(2) });
const BUTTON_2_ID: NodeId = NodeId(unsafe { NonZeroU128::new_unchecked(3) });
const ANNOUNCEMENT_ID: NodeId = NodeId(unsafe { NonZeroU128::new_unchecked(4) });
const INITIAL_FOCUS: NodeId = BUTTON_1_ID;

const BUTTON_1_RECT: Rect = Rect {
    x0: 20.0,
    y0: 20.0,
    x1: 100.0,
    y1: 60.0,
};

const BUTTON_2_RECT: Rect = Rect {
    x0: 20.0,
    y0: 60.0,
    x1: 100.0,
    y1: 100.0,
};

fn build_button(id: NodeId, name: &str, classes: &mut NodeClassSet) -> Node {
    let rect = match id {
        BUTTON_1_ID => BUTTON_1_RECT,
        BUTTON_2_ID => BUTTON_2_RECT,
        _ => unreachable!(),
    };

    let mut builder = NodeBuilder::new(Role::Button);
    builder.set_bounds(rect);
    builder.set_name(name);
    builder.add_action(Action::Focus);
    builder.set_default_action_verb(DefaultActionVerb::Click);
    builder.build(classes)
}

fn build_announcement(text: &str, classes: &mut NodeClassSet) -> Node {
    let mut builder = NodeBuilder::new(Role::StaticText);
    builder.set_name(text);
    builder.set_live(Live::Polite);
    builder.build(classes)
}

struct State {
    focus: NodeId,
    is_window_focused: bool,
    announcement: Option<String>,
    node_classes: NodeClassSet,
}

impl State {
    fn new() -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            focus: INITIAL_FOCUS,
            is_window_focused: false,
            announcement: None,
            node_classes: NodeClassSet::new(),
        }))
    }

    fn focus(&self) -> Option<NodeId> {
        self.is_window_focused.then_some(self.focus)
    }

    fn build_root(&mut self) -> Node {
        let mut builder = NodeBuilder::new(Role::Window);
        builder.set_children(vec![BUTTON_1_ID, BUTTON_2_ID]);
        if self.announcement.is_some() {
            builder.push_child(ANNOUNCEMENT_ID);
        }
        builder.set_name(WINDOW_TITLE);
        builder.build(&mut self.node_classes)
    }

    fn build_initial_tree(&mut self) -> TreeUpdate {
        let root = self.build_root();
        let button_1 = build_button(BUTTON_1_ID, "Button 1", &mut self.node_classes);
        let button_2 = build_button(BUTTON_2_ID, "Button 2", &mut self.node_classes);
        let mut result = TreeUpdate {
            nodes: vec![
                (WINDOW_ID, root),
                (BUTTON_1_ID, button_1),
                (BUTTON_2_ID, button_2),
            ],
            tree: Some(Tree::new(WINDOW_ID)),
            focus: self.focus(),
        };
        if let Some(announcement) = &self.announcement {
            result.nodes.push((
                ANNOUNCEMENT_ID,
                build_announcement(announcement, &mut self.node_classes),
            ));
        }
        result
    }

    fn update_focus(&mut self, adapter: &Adapter) {
        println!("Trying to focus -> {:?}", self.focus());
        adapter.update_if_active(|| {
            println!("Focused -> {:?}", self.focus());
            TreeUpdate {
                nodes: vec![],
                tree: None,
                focus: self.focus(),
            }
        });
    }

    fn press_button(&mut self, adapter: &Adapter, id: NodeId) {
        println!("Trying to press a button.");
        let text = if id == BUTTON_1_ID {
            "You pressed button 1"
        } else {
            "You pressed button 2"
        };
        self.announcement = Some(text.into());
        adapter.update_if_active(|| {
            let announcement = build_announcement(text, &mut self.node_classes);
            let root = self.build_root();
            println!("Pressed a button");
            TreeUpdate {
                nodes: vec![(ANNOUNCEMENT_ID, announcement), (WINDOW_ID, root)],
                tree: None,
                focus: self.focus(),
            }
        });
    }
}