use crate::utils::config::ToastClipboardPosition;
use crate::utils::render::feedback::copy_feedback_rect;
use crate::utils::render::widgets::CopyFeedback;
use ratatui::layout::Rect;

#[test]
fn copy_feedback_rect_uses_configured_position() {
    let area = Rect::new(10, 20, 100, 40);
    let feedback = CopyFeedback {
        message: "copied to clipboard".to_owned(),
    };

    let top = copy_feedback_rect(area, &feedback, 0, ToastClipboardPosition::TopCenter);
    assert_eq!(top.y, area.y);
    assert_eq!(top.x, area.x + area.width.saturating_sub(top.width) / 2);

    let bottom = copy_feedback_rect(area, &feedback, 0, ToastClipboardPosition::BottomCenter);
    assert_eq!(bottom.bottom(), area.bottom());
    assert_eq!(
        bottom.x,
        area.x + area.width.saturating_sub(bottom.width) / 2
    );
}
