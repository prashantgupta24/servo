/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

use canvas_traits::{CanvasMsg, FromLayoutMsg};
use dom::attr::Attr;
use dom::bindings::cell::DOMRefCell;
use dom::bindings::codegen::Bindings::HTMLCanvasElementBinding;
use dom::bindings::codegen::Bindings::HTMLCanvasElementBinding::HTMLCanvasElementMethods;
use dom::bindings::codegen::Bindings::WebGLRenderingContextBinding::WebGLContextAttributes;
use dom::bindings::codegen::UnionTypes::CanvasRenderingContext2DOrWebGLRenderingContext;
use dom::bindings::conversions::Castable;
use dom::bindings::global::GlobalRef;
use dom::bindings::js::{HeapGCValue, JS, LayoutJS, Root};
use dom::bindings::utils::{Reflectable};
use dom::canvasrenderingcontext2d::{CanvasRenderingContext2D, LayoutCanvasRenderingContext2DHelpers};
use dom::document::Document;
use dom::element::{AttributeMutation, Element};
use dom::htmlelement::HTMLElement;
use dom::node::{Node, window_from_node};
use dom::virtualmethods::VirtualMethods;
use dom::webglrenderingcontext::{LayoutCanvasWebGLRenderingContextHelpers, WebGLRenderingContext};
use euclid::size::Size2D;
use ipc_channel::ipc::{self, IpcSender};
use js::jsapi::{HandleValue, JSContext};
use offscreen_gl_context::GLContextAttributes;
use std::cell::Cell;
use std::iter::repeat;
use util::str::{DOMString, parse_unsigned_integer};

const DEFAULT_WIDTH: u32 = 300;
const DEFAULT_HEIGHT: u32 = 150;

#[must_root]
#[derive(JSTraceable, Clone, HeapSizeOf)]
pub enum CanvasContext {
    Context2d(JS<CanvasRenderingContext2D>),
    WebGL(JS<WebGLRenderingContext>),
}

impl HeapGCValue for CanvasContext {}

#[dom_struct]
pub struct HTMLCanvasElement {
    htmlelement: HTMLElement,
    context: DOMRefCell<Option<CanvasContext>>,
    width: Cell<u32>,
    height: Cell<u32>,
}

impl PartialEq for HTMLCanvasElement {
    fn eq(&self, other: &HTMLCanvasElement) -> bool {
        self as *const HTMLCanvasElement == &*other
    }
}

impl HTMLCanvasElement {
    fn new_inherited(localName: DOMString,
                     prefix: Option<DOMString>,
                     document: &Document) -> HTMLCanvasElement {
        HTMLCanvasElement {
            htmlelement: HTMLElement::new_inherited(localName, prefix, document),
            context: DOMRefCell::new(None),
            width: Cell::new(DEFAULT_WIDTH),
            height: Cell::new(DEFAULT_HEIGHT),
        }
    }

    #[allow(unrooted_must_root)]
    pub fn new(localName: DOMString,
               prefix: Option<DOMString>,
               document: &Document) -> Root<HTMLCanvasElement> {
        let element = HTMLCanvasElement::new_inherited(localName, prefix, document);
        Node::reflect_node(box element, document, HTMLCanvasElementBinding::Wrap)
    }

    fn recreate_contexts(&self) {
        let size = self.get_size();
        if let Some(ref context) = *self.context.borrow() {
            match *context {
                CanvasContext::Context2d(ref context) => context.recreate(size),
                CanvasContext::WebGL(ref context) => context.recreate(size),
            }
        }
    }

    pub fn get_size(&self) -> Size2D<i32> {
        Size2D::new(self.width.get() as i32, self.height.get() as i32)
    }
}

pub trait LayoutHTMLCanvasElementHelpers {
    #[allow(unsafe_code)]
    unsafe fn get_renderer_id(&self) -> Option<usize>;
    #[allow(unsafe_code)]
    unsafe fn get_ipc_renderer(&self) -> Option<IpcSender<CanvasMsg>>;
    #[allow(unsafe_code)]
    unsafe fn get_canvas_width(&self) -> u32;
    #[allow(unsafe_code)]
    unsafe fn get_canvas_height(&self) -> u32;
}

impl LayoutHTMLCanvasElementHelpers for LayoutJS<HTMLCanvasElement> {
    #[allow(unsafe_code)]
    unsafe fn get_renderer_id(&self) -> Option<usize> {
        let ref canvas = *self.unsafe_get();
        canvas.context.borrow_for_layout().as_ref().map(|context| {
            match *context {
                CanvasContext::Context2d(ref context) => context.to_layout().get_renderer_id(),
                CanvasContext::WebGL(ref context) => context.to_layout().get_renderer_id(),
            }
        })
    }

    #[allow(unsafe_code)]
    unsafe fn get_ipc_renderer(&self) -> Option<IpcSender<CanvasMsg>> {
        let ref canvas = *self.unsafe_get();
        canvas.context.borrow_for_layout().as_ref().map(|context| {
            match *context {
                CanvasContext::Context2d(ref context) => context.to_layout().get_ipc_renderer(),
                CanvasContext::WebGL(ref context) => context.to_layout().get_ipc_renderer(),
            }
        })
    }

    #[allow(unsafe_code)]
    unsafe fn get_canvas_width(&self) -> u32 {
        (*self.unsafe_get()).width.get()
    }

    #[allow(unsafe_code)]
    unsafe fn get_canvas_height(&self) -> u32 {
        (*self.unsafe_get()).height.get()
    }
}


impl HTMLCanvasElement {
    pub fn ipc_renderer(&self) -> Option<IpcSender<CanvasMsg>> {
        self.context.borrow().as_ref().map(|context| {
            match *context {
                CanvasContext::Context2d(ref context) => context.ipc_renderer(),
                CanvasContext::WebGL(ref context) => context.ipc_renderer(),
            }
        })
    }

    pub fn get_or_init_2d_context(&self) -> Option<Root<CanvasRenderingContext2D>> {
        if self.context.borrow().is_none() {
            let window = window_from_node(self);
            let size = self.get_size();
            let context = CanvasRenderingContext2D::new(GlobalRef::Window(window.r()), self, size);
            *self.context.borrow_mut() = Some(CanvasContext::Context2d(JS::from_rooted(&context)));
        }

        match *self.context.borrow().as_ref().unwrap() {
            CanvasContext::Context2d(ref context) => Some(context.root()),
            _   => None,
        }
    }

    pub fn get_or_init_webgl_context(&self,
                                 cx: *mut JSContext,
                                 attrs: Option<HandleValue>) -> Option<Root<WebGLRenderingContext>> {
        if self.context.borrow().is_none() {
            let window = window_from_node(self);
            let size = self.get_size();

            let attrs = if let Some(webgl_attributes) = attrs {
                if let Ok(ref attrs) = WebGLContextAttributes::new(cx, webgl_attributes) {
                    From::from(attrs)
                } else {
                    debug!("Unexpected error on conversion of WebGLContextAttributes");
                    return None;
                }
            } else {
                GLContextAttributes::default()
            };

            let maybe_ctx = WebGLRenderingContext::new(GlobalRef::Window(window.r()), self, size, attrs);

            *self.context.borrow_mut() = maybe_ctx.map( |ctx| CanvasContext::WebGL(JS::from_rooted(&ctx)));
        }

        if let Some(CanvasContext::WebGL(ref context)) = *self.context.borrow() {
            Some(context.root())
        } else {
            None
        }
    }

    pub fn is_valid(&self) -> bool {
        self.height.get() != 0 && self.width.get() != 0
    }

    pub fn fetch_all_data(&self) -> Option<(Vec<u8>, Size2D<i32>)> {
        let size = self.get_size();

        if size.width == 0 || size.height == 0 {
            return None
        }

        let data = if let Some(renderer) = self.ipc_renderer() {
            let (sender, receiver) = ipc::channel().unwrap();
            let msg = CanvasMsg::FromLayout(FromLayoutMsg::SendPixelContents(sender));
            renderer.send(msg).unwrap();

            receiver.recv().unwrap().to_vec()
        } else {
            repeat(0xffu8).take((size.height as usize) * (size.width as usize) * 4).collect()
        };

        Some((data, size))
    }
}

impl HTMLCanvasElementMethods for HTMLCanvasElement {
    // https://html.spec.whatwg.org/multipage/#dom-canvas-width
    fn Width(&self) -> u32 {
        self.width.get()
    }

    // https://html.spec.whatwg.org/multipage/#dom-canvas-width
    fn SetWidth(&self, width: u32) {
        self.upcast::<Element>().set_uint_attribute(&atom!("width"), width)
    }

    // https://html.spec.whatwg.org/multipage/#dom-canvas-height
    fn Height(&self) -> u32 {
        self.height.get()
    }

    // https://html.spec.whatwg.org/multipage/#dom-canvas-height
    fn SetHeight(&self, height: u32) {
        self.upcast::<Element>().set_uint_attribute(&atom!("height"), height)
    }

    // https://html.spec.whatwg.org/multipage/#dom-canvas-getcontext
    fn GetContext(&self,
                  cx: *mut JSContext,
                  id: DOMString,
                  attributes: Vec<HandleValue>)
        -> Option<CanvasRenderingContext2DOrWebGLRenderingContext> {
        match &*id {
            "2d" => {
                self.get_or_init_2d_context()
                    .map(CanvasRenderingContext2DOrWebGLRenderingContext::eCanvasRenderingContext2D)
            }
            "webgl" | "experimental-webgl" => {
                self.get_or_init_webgl_context(cx, attributes.get(0).map(|p| *p))
                    .map(CanvasRenderingContext2DOrWebGLRenderingContext::eWebGLRenderingContext)
            }
            _ => None
        }
    }
}

impl VirtualMethods for HTMLCanvasElement {
    fn super_type(&self) -> Option<&VirtualMethods> {
        Some(self.upcast::<HTMLElement>() as &VirtualMethods)
    }

    fn attribute_mutated(&self, attr: &Attr, mutation: AttributeMutation) {
        self.super_type().unwrap().attribute_mutated(attr, mutation);
        let recreate = match attr.local_name() {
            &atom!(width) => {
                let width = mutation.new_value(attr).and_then(|value| {
                    parse_unsigned_integer(value.chars())
                });
                self.width.set(width.unwrap_or(DEFAULT_WIDTH));
                true
            },
            &atom!(height) => {
                let height = mutation.new_value(attr).and_then(|value| {
                    parse_unsigned_integer(value.chars())
                });
                self.height.set(height.unwrap_or(DEFAULT_HEIGHT));
                true
            },
            _ => false,
        };
        if recreate {
            self.recreate_contexts();
        }
    }
}

impl<'a> From<&'a WebGLContextAttributes> for GLContextAttributes {
    fn from(attrs: &'a WebGLContextAttributes) -> GLContextAttributes {
        GLContextAttributes {
            alpha: attrs.alpha,
            depth: attrs.depth,
            stencil: attrs.stencil,
            antialias: attrs.antialias,
            premultiplied_alpha: attrs.premultipliedAlpha,
            preserve_drawing_buffer: attrs.preserveDrawingBuffer,
        }
    }
}

pub mod utils {
    use dom::window::Window;
    use ipc_channel::ipc;
    use net_traits::image_cache_task::{ImageCacheChan, ImageResponse};
    use url::Url;

    pub fn request_image_from_cache(window: &Window, url: Url) -> ImageResponse {
        let image_cache = window.image_cache_task();
        let (response_chan, response_port) = ipc::channel().unwrap();
        image_cache.request_image(url, ImageCacheChan(response_chan), None);
        let result = response_port.recv().unwrap();
        result.image_response
    }
}
