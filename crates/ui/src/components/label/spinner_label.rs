use crate::prelude::*;
use gpui::{Animation, AnimationExt, FontWeight};
use std::time::Duration;

/// Different types of spinner animations
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub enum SpinnerVariant {
    #[default]
    Dots,
    DotsVariant,
    Sand,
}

/// A spinner indication, based on the label component, that loops through
/// frames of the specified animation. It implements `LabelCommon` as well.
///
/// # Default Example
///
/// ```
/// use ui::{SpinnerLabel};
///
/// SpinnerLabel::new();
/// ```
///
/// # Variant Example
///
/// ```
/// use ui::{SpinnerLabel};
///
/// SpinnerLabel::dots_variant();
/// ```
#[derive(IntoElement, RegisterComponent)]
pub struct SpinnerLabel {
    base: Label,
    variant: SpinnerVariant,
}

impl SpinnerVariant {
    fn frames(&self) -> &'static [&'static str] {
        match self {
            SpinnerVariant::Dots => &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"],
            SpinnerVariant::DotsVariant => &["⣼", "⣹", "⢻", "⠿", "⡟", "⣏", "⣧", "⣶"],
            SpinnerVariant::Sand => &[
                "⠁", "⠂", "⠄", "⡀", "⡈", "⡐", "⡠", "⣀", "⣁", "⣂", "⣄", "⣌", "⣔", "⣤", "⣥", "⣦",
                "⣮", "⣶", "⣷", "⣿", "⡿", "⠿", "⢟", "⠟", "⡛", "⠛", "⠫", "⢋", "⠋", "⠍", "⡉", "⠉",
                "⠑", "⠡", "⢁",
            ],
        }
    }

    fn frame_at(&self, delta: f32) -> &'static str {
        let frames = self.frames();
        frames[(delta * frames.len() as f32) as usize % frames.len()]
    }

    fn duration(&self) -> Duration {
        match self {
            SpinnerVariant::Dots => Duration::from_millis(1000),
            SpinnerVariant::DotsVariant => Duration::from_millis(1000),
            SpinnerVariant::Sand => Duration::from_millis(2000),
        }
    }

    fn animation_id(&self) -> &'static str {
        match self {
            SpinnerVariant::Dots => "spinner_label_dots",
            SpinnerVariant::DotsVariant => "spinner_label_dots_variant",
            SpinnerVariant::Sand => "spinner_label_dots_variant_2",
        }
    }
}

impl SpinnerLabel {
    pub fn new() -> Self {
        Self::with_variant(SpinnerVariant::default())
    }

    pub fn with_variant(variant: SpinnerVariant) -> Self {
        SpinnerLabel {
            base: Label::new(variant.frame_at(0.0)).color(Color::Muted),
            variant,
        }
    }

    pub fn dots() -> Self {
        Self::with_variant(SpinnerVariant::Dots)
    }

    pub fn dots_variant() -> Self {
        Self::with_variant(SpinnerVariant::DotsVariant)
    }

    pub fn sand() -> Self {
        Self::with_variant(SpinnerVariant::Sand)
    }
}

impl LabelCommon for SpinnerLabel {
    fn size(mut self, size: LabelSize) -> Self {
        self.base = self.base.size(size);
        self
    }

    fn weight(mut self, weight: FontWeight) -> Self {
        self.base = self.base.weight(weight);
        self
    }

    fn line_height_style(mut self, line_height_style: LineHeightStyle) -> Self {
        self.base = self.base.line_height_style(line_height_style);
        self
    }

    fn color(mut self, color: Color) -> Self {
        self.base = self.base.color(color);
        self
    }

    fn strikethrough(mut self) -> Self {
        self.base = self.base.strikethrough();
        self
    }

    fn italic(mut self) -> Self {
        self.base = self.base.italic();
        self
    }

    fn alpha(mut self, alpha: f32) -> Self {
        self.base = self.base.alpha(alpha);
        self
    }

    fn underline(mut self) -> Self {
        self.base = self.base.underline();
        self
    }

    fn truncate(mut self) -> Self {
        self.base = self.base.truncate();
        self
    }

    fn single_line(mut self) -> Self {
        self.base = self.base.single_line();
        self
    }

    fn buffer_font(mut self, cx: &App) -> Self {
        self.base = self.base.buffer_font(cx);
        self
    }

    fn inline_code(mut self, cx: &App) -> Self {
        self.base = self.base.inline_code(cx);
        self
    }
}

impl RenderOnce for SpinnerLabel {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let variant = self.variant;
        let duration = variant.duration();
        let frames_per_second = variant.frames().len() as f32 / duration.as_secs_f32();

        self.base.with_animation(
            variant.animation_id(),
            Animation::new(duration)
                .repeat_synced()
                .with_max_fps(frames_per_second),
            move |mut label, delta| {
                label.set_text(variant.frame_at(delta));
                label
            },
        )
    }
}

impl Component for SpinnerLabel {
    fn scope() -> ComponentScope {
        ComponentScope::Loading
    }

    fn name() -> &'static str {
        "Spinner Label"
    }

    fn sort_name() -> &'static str {
        "Spinner Label"
    }

    fn description() -> &'static str {
        "A text-based loading indicator that animates through a sequence of \
        unicode glyphs to show ongoing or indeterminate work."
    }

    fn preview(_window: &mut Window, _cx: &mut App) -> AnyElement {
        let examples = vec![
            single_example("Default", SpinnerLabel::new().into_any_element()),
            single_example(
                "Dots Variant",
                SpinnerLabel::dots_variant().into_any_element(),
            ),
            single_example("Sand Variant", SpinnerLabel::sand().into_any_element()),
        ];

        example_group(examples).vertical().into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dots_show_every_frame_for_an_equal_part_of_the_cycle() {
        let variant = SpinnerVariant::Dots;
        let expected_frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        assert_eq!(variant.duration(), Duration::from_secs(1));
        for (index, expected) in expected_frames.into_iter().enumerate() {
            for offset in [0.0, 0.5, 0.999] {
                let delta = (index as f32 + offset) / 10.0;
                assert_eq!(variant.frame_at(delta), expected, "phase {delta}");
            }
        }
        assert_eq!(variant.frame_at(1.0), "⠋");
    }
}
