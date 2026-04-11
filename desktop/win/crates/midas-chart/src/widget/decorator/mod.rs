//! Composable badges, buttons, and flex containers anchored on `PriceLine` annotations.

pub mod action;
pub mod badge;
pub mod button;
pub mod compute;
pub mod group;
pub mod layout;

#[cfg(test)]
mod tests;

pub use self::action::DecoratorAction;
pub use self::badge::{Badge, BadgeBorder, BadgeSegment, BadgeShape};
pub use self::button::Button;
pub use self::compute::{
    compute_decorator_group, recompute_decorator_hit_zones, rect_contains, DecoratorGroupRef,
};
pub use self::group::{
    DecoratorAnchor, DecoratorGroup, DecoratorItem, FlexDirection, ItemContent, Visibility,
};
