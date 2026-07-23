//! Segmented pill control: a row of linked toggle buttons, one active at a
//! time. Used for short option lists; longer ones use `AdwComboRow` instead.

use gtk::prelude::*;

/// `on_change` fires with the newly selected index, not on initial build.
pub fn build(options: &[&str], selected: usize, on_change: impl Fn(usize) + 'static) -> gtk::Box {
    let container = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(0)
        .valign(gtk::Align::Center)
        .vexpand(false)
        .css_classes(["linked"])
        .build();

    let on_change = std::rc::Rc::new(on_change);
    let mut first_button: Option<gtk::ToggleButton> = None;

    for (i, label) in options.iter().enumerate() {
        let button = gtk::ToggleButton::builder()
            .label(*label)
            .valign(gtk::Align::Center)
            .vexpand(false)
            .css_classes(["segmented-pill"])
            .build();
        if i == selected {
            button.set_active(true);
            button.add_css_class("suggested-action");
        }
        if let Some(ref first) = first_button {
            button.set_group(Some(first));
        } else {
            first_button = Some(button.clone());
        }

        let on_change = on_change.clone();
        let container_ref = container.clone();
        button.connect_toggled(move |b| {
            if !b.is_active() {
                return;
            }
            // Only the newly-active button should carry the accent color —
            // toggle-group semantics only guarantee exclusivity, not this
            // per-button "look pressed" styling, so we maintain it by hand.
            let mut child = container_ref.first_child();
            while let Some(widget) = child {
                widget.remove_css_class("suggested-action");
                child = widget.next_sibling();
            }
            b.add_css_class("suggested-action");
            on_change(i);
        });

        container.append(&button);
    }

    container
}
