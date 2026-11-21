//! Dragging on the Map (#122), in the page's own code: a device's box onto
//! another place, onto "new place", onto "no place set", or up and down in
//! its place; a place's name left or right among the places. Mouse only: a
//! finger on the schema scrolls the page. The schema is drawn again on every
//! change, so each move asks the DOM afresh; the drag's own marks (its name
//! at the pointer, the line where it lands, the "new place" target) live on
//! `body`.

use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{Element, PointerEvent};

/// What a drop asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dropped {
    /// `device` into `place` ("" for none), before the device `before`
    /// there ("" for last; `None`: where it stands now).
    Device {
        device: String,
        place: String,
        before: Option<String>,
    },
    /// `place` before the place `before` ("" for last).
    Place { place: String, before: String },
}

/// A box on the screen: its name and its top and bottom.
#[derive(Debug, Clone, PartialEq)]
pub struct Stack {
    pub name: String,
    pub top: f64,
    pub bottom: f64,
}

/// Where in a place's column a box let go at height `y` stands: before the
/// first box whose middle is below `y`, else last (""). The boxes in the
/// column's order, the dragged one left out.
pub fn before_at(boxes: &[Stack], y: f64) -> String {
    boxes
        .iter()
        .find(|b| (b.top + b.bottom) / 2.0 > y)
        .map_or_else(String::new, |b| b.name.clone())
}

/// An area on the screen: its place ("" for "no place set") and its left
/// and right.
#[derive(Debug, Clone, PartialEq)]
pub struct Column {
    pub place: String,
    pub left: f64,
    pub right: f64,
}

/// Where a place's name let go at `x` puts it: before the first named area
/// whose middle is right of `x`, else last (""). "no place set" stays last.
pub fn place_before_at(areas: &[Column], x: f64) -> String {
    let mut named: Vec<&Column> = areas.iter().filter(|a| !a.place.is_empty()).collect();
    named.sort_by(|a, b| a.left.total_cmp(&b.left));
    named
        .into_iter()
        .find(|a| (a.left + a.right) / 2.0 > x)
        .map_or_else(String::new, |a| a.place.clone())
}

/// The longest place name kept, as the server keeps it.
pub fn new_place_name(typed: &str) -> Option<String> {
    let name: String = typed
        .trim()
        .chars()
        .take(dcontroller_api::PLACE_LEN)
        .collect();
    (!name.is_empty()).then_some(name)
}

enum What {
    Device(String),
    Place(String),
}

struct Drag {
    what: What,
    wrap: Element,
    x: f64,
    y: f64,
    /// Begun: the pointer went 8 px from where it was pressed.
    marks: Option<Marks>,
}

struct Marks {
    ghost: Element,
    line: Element,
    /// Only for a device: the "new place" target.
    zone: Option<Element>,
}

/// Where the pointer is now, and the line that shows it.
enum Target {
    New,
    Area {
        place: String,
        before: String,
        area: Element,
        line: (f64, f64, f64),
    },
    Slot {
        before: String,
        line: (f64, f64, f64),
    },
}

fn rect(e: &Element) -> web_sys::DomRect {
    e.get_bounding_client_rect()
}

fn all(root: &Element, sel: &str) -> Vec<Element> {
    let Ok(list) = root.query_selector_all(sel) else {
        return Vec::new();
    };
    (0..list.length())
        .filter_map(|i| list.item(i).and_then(|n| n.dyn_into::<Element>().ok()))
        .collect()
}

fn mark(class: &str, text: &str) -> Option<Element> {
    let doc = web_sys::window()?.document()?;
    let e = doc.create_element("div").ok()?;
    e.set_class_name(class);
    e.set_text_content((!text.is_empty()).then_some(text));
    doc.body()?.append_child(&e).ok()?;
    Some(e)
}

fn style(e: &Element, css: &str) {
    let _ = e.set_attribute("style", css);
}

fn inside(r: &web_sys::DomRect, x: f64, y: f64) -> bool {
    x >= r.left() && x <= r.right() && y >= r.top() && y <= r.bottom()
}

fn target(d: &Drag, x: f64, y: f64) -> Option<Target> {
    match &d.what {
        What::Device(name) => {
            if let Some(z) = d.marks.as_ref().and_then(|m| m.zone.as_ref())
                && inside(&rect(z), x, y)
            {
                return Some(Target::New);
            }
            for area in all(&d.wrap, "svg.map rect.area") {
                let r = rect(&area);
                if x < r.left() || x > r.right() || y < r.top() - 20.0 || y > r.bottom() + 20.0 {
                    continue;
                }
                let place = area.get_attribute("data-place").unwrap_or_default();
                let mut boxes: Vec<Stack> = all(&d.wrap, "svg.map g.node:not(.asio)")
                    .into_iter()
                    .filter(|n| n.get_attribute("data-place").unwrap_or_default() == place)
                    .filter(|n| n.get_attribute("data-node").as_deref() != Some(name.as_str()))
                    .filter_map(|n| {
                        let b = rect(&n.query_selector("rect").ok()??);
                        Some(Stack {
                            name: n.get_attribute("data-node")?,
                            top: b.top(),
                            bottom: b.bottom(),
                        })
                    })
                    .collect();
                boxes.sort_by(|a, b| a.top.total_cmp(&b.top));
                let before = before_at(&boxes, y);
                let at = match boxes.iter().find(|b| b.name == before) {
                    Some(b) if !before.is_empty() => b.top - 8.0,
                    _ => boxes.last().map_or(r.top() + 40.0, |b| b.bottom + 8.0),
                };
                return Some(Target::Area {
                    place,
                    before,
                    area,
                    line: (r.left() + 8.0, at, r.width() - 16.0),
                });
            }
            None
        }
        What::Place(place) => {
            let areas: Vec<(Column, web_sys::DomRect)> = all(&d.wrap, "svg.map rect.area")
                .into_iter()
                .map(|a| {
                    let r = rect(&a);
                    (
                        Column {
                            place: a.get_attribute("data-place").unwrap_or_default(),
                            left: r.left(),
                            right: r.right(),
                        },
                        r,
                    )
                })
                .filter(|(c, _)| c.place != *place)
                .collect();
            let columns: Vec<Column> = areas.iter().map(|(c, _)| c.clone()).collect();
            let before = place_before_at(&columns, x);
            let (left, r) = match areas
                .iter()
                .find(|(c, _)| c.place == before && !before.is_empty())
            {
                Some((_, r)) => (r.left() - 14.0, r),
                None => {
                    let last = areas
                        .iter()
                        .filter(|(c, _)| !c.place.is_empty())
                        .max_by(|a, b| a.0.left.total_cmp(&b.0.left))
                        .or_else(|| areas.first())?;
                    (last.1.right() + 11.0, &last.1)
                }
            };
            Some(Target::Slot {
                before,
                line: (left, r.top(), r.height()),
            })
        }
    }
}

fn show(d: &Drag, t: Option<&Target>) {
    let Some(m) = d.marks.as_ref() else {
        return;
    };
    for a in all(&d.wrap, "svg.map rect.area.drop") {
        let _ = a.class_list().remove_1("drop");
    }
    if let Some(z) = &m.zone {
        let _ = z
            .class_list()
            .toggle_with_force("drop", matches!(t, Some(Target::New)));
    }
    match t {
        Some(Target::Area {
            place, area, line, ..
        }) => {
            let _ = area.class_list().add_1("drop");
            // In "no place set" the order is the map's: no line there.
            if place.is_empty() {
                style(&m.line, "display:none");
            } else {
                style(
                    &m.line,
                    &format!(
                        "display:block;left:{}px;top:{}px;width:{}px",
                        line.0, line.1, line.2
                    ),
                );
            }
        }
        Some(Target::Slot { line, .. }) => style(
            &m.line,
            &format!(
                "display:block;left:{}px;top:{}px;width:3px;height:{}px",
                line.0, line.1, line.2
            ),
        ),
        _ => style(&m.line, "display:none"),
    }
}

fn begin(d: &mut Drag) {
    let name = match &d.what {
        What::Device(n) | What::Place(n) => n.clone(),
    };
    let (Some(ghost), Some(line)) = (mark("map-drag-ghost", &name), mark("map-drag-line", ""))
    else {
        return;
    };
    let zone = match d.what {
        What::Device(_) => mark("map-drag-new", "+ new place").inspect(|z| {
            let _ = z.set_attribute("data-testid", "map-new-place");
            let w = rect(&d.wrap);
            let height = web_sys::window()
                .and_then(|w| w.inner_height().ok())
                .and_then(|h| h.as_f64())
                .unwrap_or(800.0);
            let top = w.top().max(8.0);
            let h = (w.bottom().min(height - 8.0) - top).max(60.0);
            style(
                z,
                &format!("left:{}px;top:{top}px;height:{h}px", w.right() - 150.0),
            );
        }),
        What::Place(_) => None,
    };
    if let Some(b) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.body())
    {
        let _ = b.class_list().add_1("map-dragging");
    }
    d.marks = Some(Marks { ghost, line, zone });
}

fn end(d: &Drag) {
    if let Some(m) = &d.marks {
        m.ghost.remove();
        m.line.remove();
        if let Some(z) = &m.zone {
            z.remove();
        }
    }
    for a in all(&d.wrap, "svg.map rect.area.drop") {
        let _ = a.class_list().remove_1("drop");
    }
    if let Some(b) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.body())
    {
        let _ = b.class_list().remove_1("map-dragging");
    }
}

/// Listens on the page for drags on the schema inside `wrap` and hands each
/// drop to `on_drop`. Installed once; the listeners live with the page.
pub fn install(wrap: Element, on_drop: impl Fn(Dropped) + 'static) {
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let state: Rc<RefCell<Option<Drag>>> = Rc::default();
    // The click that ends a drag is no choice of the box under it.
    let swallow: Rc<RefCell<bool>> = Rc::default();
    let on_drop = Rc::new(on_drop);

    let down = {
        let state = Rc::clone(&state);
        Closure::<dyn Fn(PointerEvent)>::new(move |e: PointerEvent| {
            if e.button() != 0 || e.pointer_type() != "mouse" {
                return;
            }
            let Some(t) = e.target().and_then(|t| t.dyn_into::<Element>().ok()) else {
                return;
            };
            if !wrap.contains(Some(&t)) {
                return;
            }
            let what = if let Ok(Some(title)) = t.closest("svg.map text.col-title[data-place]") {
                What::Place(title.get_attribute("data-place").unwrap_or_default())
            } else if let Ok(Some(node)) = t.closest("svg.map g.node:not(.asio)") {
                What::Device(node.get_attribute("data-node").unwrap_or_default())
            } else {
                return;
            };
            *state.borrow_mut() = Some(Drag {
                what,
                wrap: wrap.clone(),
                x: f64::from(e.client_x()),
                y: f64::from(e.client_y()),
                marks: None,
            });
        })
    };
    let moved = {
        let state = Rc::clone(&state);
        Closure::<dyn Fn(PointerEvent)>::new(move |e: PointerEvent| {
            let mut s = state.borrow_mut();
            let Some(d) = s.as_mut() else {
                return;
            };
            let (x, y) = (f64::from(e.client_x()), f64::from(e.client_y()));
            if d.marks.is_none() {
                if (x - d.x).hypot(y - d.y) < 8.0 {
                    return;
                }
                begin(d);
            }
            e.prevent_default();
            if let Some(m) = &d.marks {
                style(&m.ghost, &format!("left:{}px;top:{}px", x + 12.0, y + 12.0));
            }
            let t = target(d, x, y);
            show(d, t.as_ref());
        })
    };
    let up = {
        let state = Rc::clone(&state);
        let swallow = Rc::clone(&swallow);
        let on_drop = Rc::clone(&on_drop);
        Closure::<dyn Fn(PointerEvent)>::new(move |e: PointerEvent| {
            let Some(d) = state.borrow_mut().take() else {
                return;
            };
            if d.marks.is_none() {
                return;
            }
            let t = (e.type_() == "pointerup")
                .then(|| target(&d, f64::from(e.client_x()), f64::from(e.client_y())))
                .flatten();
            end(&d);
            *swallow.borrow_mut() = true;
            let s = Rc::clone(&swallow);
            leptos::prelude::set_timeout(
                move || *s.borrow_mut() = false,
                std::time::Duration::ZERO,
            );
            let dropped = match (d.what, t) {
                (What::Place(place), Some(Target::Slot { before, .. })) => {
                    Some(Dropped::Place { place, before })
                }
                (What::Device(device), Some(Target::New)) => web_sys::window()
                    .and_then(|w| {
                        w.prompt_with_message(&format!("A new place for {device}: its name"))
                            .ok()
                            .flatten()
                    })
                    .and_then(|typed| new_place_name(&typed))
                    .map(|place| Dropped::Device {
                        device,
                        place,
                        before: Some(String::new()),
                    }),
                (What::Device(device), Some(Target::Area { place, before, .. })) => {
                    Some(if place.is_empty() {
                        Dropped::Device {
                            device,
                            place,
                            before: None,
                        }
                    } else {
                        Dropped::Device {
                            device,
                            place,
                            before: Some(before),
                        }
                    })
                }
                _ => None,
            };
            if let Some(x) = dropped {
                on_drop(x);
            }
        })
    };
    let click = {
        let swallow = Rc::clone(&swallow);
        Closure::<dyn Fn(web_sys::MouseEvent)>::new(move |e: web_sys::MouseEvent| {
            if std::mem::take(&mut *swallow.borrow_mut()) {
                e.stop_propagation();
                e.prevent_default();
            }
        })
    };
    let on = |kind: &str, f: &js_sys::Function| {
        let _ = doc.add_event_listener_with_callback(kind, f);
    };
    on("pointerdown", down.as_ref().unchecked_ref());
    on("pointermove", moved.as_ref().unchecked_ref());
    on("pointerup", up.as_ref().unchecked_ref());
    on("pointercancel", up.as_ref().unchecked_ref());
    let _ = doc.add_event_listener_with_callback_and_bool(
        "click",
        click.as_ref().unchecked_ref(),
        true,
    );
    down.forget();
    moved.forget();
    up.forget();
    click.forget();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(name: &str, top: f64) -> Stack {
        Stack {
            name: name.into(),
            top,
            bottom: top + 58.0,
        }
    }

    #[test]
    fn a_box_let_go_stands_before_the_first_box_below_the_pointer() {
        let col = [b("desk", 100.0), b("amp", 200.0), b("rack", 300.0)];
        assert_eq!(before_at(&col, 50.0), "desk");
        assert_eq!(before_at(&col, 140.0), "amp");
        assert_eq!(before_at(&col, 240.0), "rack");
        assert_eq!(before_at(&col, 400.0), "");
        assert_eq!(before_at(&[], 10.0), "");
    }

    #[test]
    fn a_place_let_go_stands_before_the_first_area_right_of_the_pointer() {
        let c = |place: &str, left: f64| Column {
            place: place.into(),
            left,
            right: left + 200.0,
        };
        let areas = [c("STAGE", 400.0), c("FOH1", 0.0), c("", 800.0)];
        assert_eq!(place_before_at(&areas, 5.0), "FOH1");
        assert_eq!(place_before_at(&areas, 150.0), "STAGE");
        // Right of every named place, even over "no place set": last.
        assert_eq!(place_before_at(&areas, 900.0), "");
    }

    #[test]
    fn a_new_place_name_is_trimmed_and_kept_short() {
        assert_eq!(new_place_name("  Stage "), Some("Stage".into()));
        assert_eq!(new_place_name("   "), None);
        assert_eq!(new_place_name(&"x".repeat(50)).map(|n| n.len()), Some(40));
    }
}
