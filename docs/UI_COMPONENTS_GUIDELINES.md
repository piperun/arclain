# UI Components & Widgets Usage Guidelines

This document provides guidelines for using the shared components and widgets in the Arclain UI codebase. **Always prefer using these reusable components over raw egui widgets** to ensure consistency, maintainability, and adherence to the Y2K design aesthetic.

---

## Table of Contents

1. [Core Principles](#core-principles)
2. [Available Widgets](#available-widgets-crateswidgets)
3. [Available Shared Components](#available-shared-components-cratesuisrcsharedcomponents)
4. [Component Selection Guide](#component-selection-guide)
5. [Settings Page Patterns](#settings-page-patterns)
6. [Anti-Patterns to Avoid](#anti-patterns-to-avoid)
7. [Migration Checklist](#migration-checklist)

---

## Core Principles

### 1. Always Use Shared Components First

Before writing any UI code, check if a shared component or widget already exists for your use case. This ensures:

- **Consistent styling** across the application
- **Automatic theme support** (Y2K aesthetic, color schemes)
- **Reduced code duplication**
- **Easier maintenance** when design changes

### 2. Theme Integration

All components use `ThemeColors` for consistent styling. Use the theme system:

```rust
use arclain_widgets::text::{get_theme, set_theme};

// Components automatically use theme when available
let text = Text::new("Hello").muted();  // Uses theme secondary color
```

### 3. Builder Pattern

Most components use a fluent builder pattern:

```rust
TextButton::new("Save")
    .variant(ButtonVariant::Primary)
    .size(ButtonSize::Medium)
    .ui(ui);
```

---

## Available Widgets (`crates/widgets/`)

### Text

**Purpose**: Primary text rendering with automatic theme support and pixel alignment.

**Use instead of**: `ui.label()`, `RichText::new()`

```rust
use arclain_widgets::text::Text;

// Basic text
Text::new("Hello World").show(ui);

// Styled text
Text::new("Muted text").muted().show(ui);
Text::new("Bold text").strong().show(ui);
Text::new("Code").monospace().show(ui);
Text::new("Custom").size(18.0).color(Color32::RED).show(ui);
```

### TextButton

**Purpose**: Standardized button with semantic variants and sizing.

**Use instead of**: `ui.button()`, `egui::Button::new()`

```rust
use arclain_widgets::button::{TextButton, ButtonVariant, ButtonSize};

// Primary action button
TextButton::new("Save")
    .variant(ButtonVariant::Primary)
    .size(ButtonSize::Medium)
    .ui(ui);

// Secondary/Cancel button
TextButton::new("Cancel")
    .variant(ButtonVariant::Secondary)
    .ui(ui);

// Ghost button (minimal styling)
TextButton::new("Skip")
    .variant(ButtonVariant::Ghost)
    .ui(ui);
```

**Size Presets**:
- `Small`: 60×28px
- `Medium`: 80×32px (default)
- `Large`: 100×40px
- `XLarge`: 120×48px

### IconButton

**Purpose**: Icon-only buttons for toolbar actions.

**Use instead of**: Raw icon buttons with manual styling

```rust
use arclain_widgets::icon_button::{IconButton, IconButtonSize};

IconButton::new(egui_phosphor::regular::GEAR)
    .size(IconButtonSize::Medium)
    .ui(ui);
```

### ToggleSwitch

**Purpose**: Animated on/off toggle with Y2K styling.

**Use instead of**: `ui.checkbox()`

```rust
use arclain_widgets::toggle_switch::ToggleSwitch;

let mut enabled = true;
if ToggleSwitch::new(&mut enabled)
    .with_theme_colors()
    .ui(ui)
    .changed()
{
    // Handle state change
}

// With labels
ToggleSwitch::new(&mut enabled)
    .text("ON", "OFF")
    .with_theme_colors()
    .ui(ui);
```

### ToggleButton

**Purpose**: Button with selected/unselected state for toolbar toggles.

```rust
use arclain_widgets::toggle_button::ToggleButton;

let mut selected = false;
ToggleButton::new(egui_phosphor::regular::LIST, &mut selected).ui(ui);
```

### SegmentedControl

**Purpose**: Two-option selector (like iOS UISegmentedControl).

```rust
use arclain_widgets::segmented_control::SegmentedControl;

let mut use_grid = true;
SegmentedControl::new(&mut use_grid)
    .labels("Grid", "List")
    .with_theme_colors()
    .ui(ui);
```

### ThemedSlider

**Purpose**: Slider with editable value text (Y2K style).

```rust
use arclain_widgets::themed_slider::ThemedSlider;

let mut value = 50.0;
ThemedSlider::new(&mut value, 0.0..=100.0)
    .suffix("%")
    .width(200.0)
    .with_theme_colors()
    .ui(ui);
```

### CollapsibleSection

**Purpose**: Expandable/collapsible sections with header.

```rust
use arclain_widgets::collapsible_section::CollapsibleSection;

CollapsibleSection::new("Advanced Options")
    .default_open(false)
    .show(ui, |ui| {
        // Section content
    });
```

### Chips (Badge)

**Purpose**: Pill-shaped labels/badges.

```rust
use arclain_widgets::chips::Chips;

Chips::new("Tag").ui(ui);
Chips::new("Custom")
    .background_color(Color32::from_rgb(100, 150, 200))
    .ui(ui);
```

### Toast

**Purpose**: Temporary notification messages.

```rust
use arclain_widgets::toast::{Toast, Toaster};

// Create toaster (store in state)
let mut toaster = Toaster::new();

// Show notifications
toaster.add(Toast::success("File saved!"));
toaster.add(Toast::error("Failed to connect"));
toaster.add(Toast::warning("Large file detected").with_duration(Duration::from_secs(5)));

// Render in UI loop
toaster.show(ctx);
```

---

## Available Shared Components (`crates/ui/src/shared/components/`)

### Settings Form Components

Use these for ALL settings pages:

```rust
use crate::shared::components::settings_form::{
    SettingsForm, SettingsGroup, SettingsRow, SectionHeader
};
```

#### SettingsForm

**Purpose**: Scrollable container with proper margins for settings pages.

```rust
SettingsForm::new().show(ui, |ui| {
    // Settings content
});
```

#### SectionHeader

**Purpose**: Section title with theme-aware styling.

```rust
SectionHeader::new("Appearance").show(ui);
```

#### SettingsRow

**Purpose**: Standardized row with title, optional description, and action widget.

```rust
SettingsRow::new("Enable dark mode")
    .description("Use dark colors for the interface")
    .show(ui, |ui| {
        ToggleSwitch::new(&mut enabled).ui(ui);
    });
```

#### SettingsGroup

**Purpose**: Bordered container for grouped settings (Y2K style).

```rust
SettingsGroup::new().show(ui, |ui| {
    // Grouped settings rows
});
```

### Settings Header

**Purpose**: Header with icon, title, description, and action buttons.

```rust
use crate::shared::components::settings_header::SettingsHeader;

SettingsHeader::new("General Settings")
    .icon(egui_phosphor::regular::GEAR)
    .description("Configure general application behavior")
    .back_action(|| { /* navigate back */ })
    .save_action(has_changes, || { /* save */ })
    .show(ui);
```

### Breadcrumbs

**Purpose**: Navigation trail with clickable items.

```rust
use crate::shared::components::breadcrumbs::Breadcrumbs;

let items = vec![
    ("Settings".to_string(), AppPage::Settings),
    ("Interface".to_string(), AppPage::InterfaceSettings),
];

if let Some(page) = Breadcrumbs::render(ui, &items) {
    // Navigate to page
}
```

### Panel

**Purpose**: Reusable container with header, body, and footer.

```rust
use crate::shared::components::panel::{
    Panel, PanelHeader, PanelBody, PanelFooter, PanelAction
};

let header = PanelHeader::new()
    .title("Properties")
    .icon(egui_phosphor::regular::INFO);

let body = vec![
    PanelBody::Properties(properties),
];

let footer = PanelFooter::new()
    .primary_button("Save", || {});

match Panel::new()
    .header(header)
    .body(body)
    .footer(footer)
    .show(ui)
{
    PanelAction::FooterAction(idx) => { /* handle */ }
    _ => {}
}
```

### Network Log

**Purpose**: Display network activity logs.

```rust
use crate::shared::components::network_log::NetworkLog;

NetworkLog::render(ui, &log_entries);
// Or compact version
NetworkLog::render_compact(ui, &log_entries, 200.0);
```

### Status Icon

**Purpose**: Small indicator with icon, label, and count.

```rust
use crate::shared::components::status_icon::StatusIcon;

StatusIcon::new(egui_phosphor::regular::PLUGIN)
    .label("Plugins")
    .count(Some((3, 5)))
    .tooltip("3 of 5 plugins active")
    .show(ui);
```

### Preview Tree

**Purpose**: Hierarchical file/folder structure viewer.

```rust
use crate::shared::components::preview_tree::{PreviewTree, PreviewTreeState};

let mut tree_state = PreviewTreeState::default();
let tree = PreviewTree::build_tree_from_paths(&paths);
PreviewTree::render(ui, &tree, &mut tree_state, PreviewTreeFilter::All);
```

### Toolbar

**Purpose**: Configurable toolbar for file operations.

```rust
use crate::shared::components::toolbar::{Toolbar, ToolbarConfig, ToolbarState};

let actions = Toolbar::render(ui, &config, &mut state);
if actions.open { /* handle open */ }
if actions.extract { /* handle extract */ }
```

### Context Menu

**Purpose**: Right-click menu for file operations.

```rust
use crate::shared::components::context_menu::ContextMenu;

response.context_menu(|ui| {
    if let Some(action) = ContextMenu::render(ui, &items, has_selection) {
        // Handle action
    }
});
```

### Search Bar

**Purpose**: Searchable input field.

```rust
use crate::shared::components::search_bar::SearchBar;

let mut query = String::new();
SearchBar::new(&mut query)
    .hint("Search files...")
    .desired_width(200.0)
    .show(ui);
```

---

## Component Selection Guide

| Need | Use This | NOT This |
|------|----------|----------|
| Display text | `Text::new()` | `ui.label()` |
| Primary action button | `TextButton::new().variant(Primary)` | `ui.button()` |
| Cancel/secondary button | `TextButton::new().variant(Secondary)` | `ui.button()` |
| Icon-only button | `IconButton::new()` | `egui::Button::image()` |
| Boolean toggle | `ToggleSwitch::new()` | `ui.checkbox()` |
| Two-option selector | `SegmentedControl::new()` | Two buttons |
| Numeric slider | `ThemedSlider::new()` | `ui.add(egui::Slider)` |
| Settings page layout | `SettingsForm` + `SettingsRow` | Manual `ui.horizontal()` |
| Settings section title | `SectionHeader::new()` | `ui.heading()` |
| Grouped settings | `SettingsGroup::new()` | `egui::Frame` |
| Page header with actions | `SettingsHeader::new()` | Manual layout |
| Collapsible content | `CollapsibleSection::new()` | `egui::CollapsingHeader` |
| Notifications | `Toast::success()` / `Toast::error()` | Custom popup |
| Badges/tags | `Chips::new()` | Manual `egui::Frame` |

---

## Settings Page Patterns

### Correct Pattern (Follow This)

```rust
use crate::shared::components::settings_form::{
    SettingsForm, SettingsGroup, SettingsRow, SectionHeader
};
use arclain_widgets::{
    toggle_switch::ToggleSwitch,
    button::TextButton,
    themed_slider::ThemedSlider,
};

pub fn render_settings_page(ui: &mut Ui, state: &mut SettingsState) {
    SettingsForm::new().show(ui, |ui| {
        // Section 1
        SectionHeader::new("Appearance").show(ui);

        SettingsGroup::new().show(ui, |ui| {
            SettingsRow::new("Dark mode")
                .description("Use dark colors")
                .show(ui, |ui| {
                    ToggleSwitch::new(&mut state.dark_mode)
                        .with_theme_colors()
                        .ui(ui);
                });

            SettingsRow::new("Font size")
                .show(ui, |ui| {
                    ThemedSlider::new(&mut state.font_size, 10.0..=24.0)
                        .suffix("px")
                        .with_theme_colors()
                        .ui(ui);
                });
        });

        ui.add_space(16.0);

        // Section 2
        SectionHeader::new("Behavior").show(ui);

        SettingsGroup::new().show(ui, |ui| {
            SettingsRow::new("Auto-save")
                .description("Save changes automatically")
                .show(ui, |ui| {
                    ToggleSwitch::new(&mut state.auto_save)
                        .with_theme_colors()
                        .ui(ui);
                });
        });
    });
}
```

### Reference Files

For examples of correct usage, see:

- **Best Example**: [pages/network.rs](crates/ui/src/features/settings/presentation/pages/network.rs)
- **Widget Usage**: [pages/interface/sections/layout_section.rs](crates/ui/src/features/settings/presentation/pages/interface/sections/layout_section.rs)
- **Toggle Switches**: [pages/interface/sections/context_menu_section.rs](crates/ui/src/features/settings/presentation/pages/interface/sections/context_menu_section.rs)

---

## Anti-Patterns to Avoid

### ❌ Raw Checkbox Instead of ToggleSwitch

```rust
// BAD
ui.checkbox(&mut enabled, "Enable feature");

// GOOD
SettingsRow::new("Enable feature").show(ui, |ui| {
    ToggleSwitch::new(&mut enabled).with_theme_colors().ui(ui);
});
```

### ❌ Raw Button Instead of TextButton

```rust
// BAD
if ui.button("Save").clicked() { }
if egui::Button::new("Reset").min_size(vec2(80.0, 32.0)).ui(ui).clicked() { }

// GOOD
if TextButton::new("Save").variant(ButtonVariant::Primary).ui(ui).clicked() { }
if TextButton::new("Reset").variant(ButtonVariant::Secondary).ui(ui).clicked() { }
```

### ❌ Manual Frame Styling

```rust
// BAD
egui::Frame::none()
    .fill(ui.visuals().window_fill())
    .stroke(Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color))
    .inner_margin(Margin::same(12.0))
    .rounding(Rounding::ZERO)
    .show(ui, |ui| { });

// GOOD
SettingsGroup::new().show(ui, |ui| { });
```

### ❌ Manual Section Headers

```rust
// BAD
ui.heading("Section Title");
ui.add_space(8.0);

// GOOD
SectionHeader::new("Section Title").show(ui);
```

### ❌ Duplicating Layout Logic

```rust
// BAD (duplicating settings row layout)
ui.horizontal(|ui| {
    ui.vertical(|ui| {
        ui.label("Setting name");
        ui.label(RichText::new("Description").small().weak());
    });
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        // widget
    });
});

// GOOD
SettingsRow::new("Setting name")
    .description("Description")
    .show(ui, |ui| { /* widget */ });
```

### ❌ Colored Labels for Status

```rust
// BAD
ui.colored_label(Color32::RED, "Error: Connection failed");

// GOOD
toaster.add(Toast::error("Connection failed"));
// Or use StatusIcon for persistent status
```

---

## Migration Checklist

When reviewing or refactoring settings pages, check for:

- [ ] `ui.checkbox()` → Replace with `ToggleSwitch`
- [ ] `ui.button()` or `egui::Button::new()` → Replace with `TextButton`
- [ ] `ui.heading()` → Replace with `SectionHeader`
- [ ] `egui::Frame` for sections → Replace with `SettingsGroup`
- [ ] Manual horizontal layouts for settings → Replace with `SettingsRow`
- [ ] Scrollable settings content → Wrap in `SettingsForm`
- [ ] `egui::Slider` → Replace with `ThemedSlider`
- [ ] `egui::CollapsingHeader` → Replace with `CollapsibleSection`
- [ ] Custom chip/badge rendering → Use `Chips` widget
- [ ] Status messages → Use `Toast` or `StatusIcon`

---

## Files Needing Migration

The following settings files are currently NOT using shared components and need refactoring:

### High Priority

| File | Issues |
|------|--------|
| [pages/archives.rs](crates/ui/src/features/settings/presentation/pages/archives.rs) | Uses raw `ui.checkbox()`, `egui::TextEdit`, `egui::Button`, `egui::ComboBox` |
| [pages/security.rs](crates/ui/src/features/settings/presentation/pages/security.rs) | Uses raw `egui::TextEdit`, `egui::Button`, `egui::ComboBox`, colored labels |
| [pages/keyboard_mouse.rs](crates/ui/src/features/settings/presentation/pages/keyboard_mouse.rs) | All raw egui, no shared components |
| [pages/organization_rules.rs](crates/ui/src/features/settings/presentation/pages/organization_rules.rs) | All raw egui, no form components |
| [pages/general.rs](crates/ui/src/features/settings/presentation/pages/general.rs) | Uses raw `ui.checkbox()` |

### Medium Priority

| File | Issues |
|------|--------|
| [pages/interface/interface.rs](crates/ui/src/features/settings/presentation/pages/interface/interface.rs) | Mixed usage, dialog buttons are raw |
| [views/navigation.rs](crates/ui/src/features/settings/presentation/views/navigation.rs) | Card styling duplicated, manual hover effects |
| [views/header.rs](crates/ui/src/features/settings/presentation/views/header.rs) | Reset buttons are raw egui |

### Already Compliant ✓

| File | Status |
|------|--------|
| [pages/network.rs](crates/ui/src/features/settings/presentation/pages/network.rs) | Best example - uses `SettingsForm`, `SettingsGroup`, `SettingsRow` |
| [pages/interface/sections/layout_section.rs](crates/ui/src/features/settings/presentation/pages/interface/sections/layout_section.rs) | Proper use of all widgets |
| [pages/interface/sections/context_menu_section.rs](crates/ui/src/features/settings/presentation/pages/interface/sections/context_menu_section.rs) | Good use of `ToggleSwitch` |
| [pages/interface/sections/info_panel_section.rs](crates/ui/src/features/settings/presentation/pages/interface/sections/info_panel_section.rs) | Good use of `ToggleSwitch` |

---

## Additional Recommendations

### 1. Create Themed ComboBox Wrapper

Currently no shared component exists for dropdown selection. Consider creating:

```rust
// Proposed: crates/widgets/src/dropdown.rs
pub struct ThemedDropdown<T> {
    selected: T,
    options: Vec<(T, String)>,
}
```

### 2. Extract Navigation Card Component

The settings navigation uses duplicated card styling. Extract into:

```rust
// Proposed: crates/ui/src/shared/components/settings_card.rs
pub struct SettingsCard {
    title: String,
    description: String,
    icon: IconData,
}
```

### 3. Extract Chip Component for Layout Editors

Both `toolbar_layout.rs` and `info_panel_layout.rs` implement identical chip styling. This should be extracted or use the existing `Chips` widget.

---

## Summary

**Golden Rule**: Before using any raw egui widget, check if a shared component or widget exists. If it does, use it. If it doesn't and you find yourself duplicating styling logic, consider extracting a new component.

Following these guidelines ensures:
- ✅ Consistent Y2K aesthetic across the application
- ✅ Automatic theme support
- ✅ Reduced code duplication
- ✅ Easier maintenance and design updates
- ✅ Better developer experience
