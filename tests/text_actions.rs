//! Text actions against real inputs: a generated QR code and rendered text.

use krab::geom::RectF;
use krab::gfx::{argb_to_rgba, text, Canvas, Color};
use krab::ocr;
use krab::output::Image;

fn qr_image(payload: &str, module: u32) -> Image {
    let code = qrcode::QrCode::new(payload.as_bytes()).unwrap();
    let n = code.width() as u32;
    let quiet = 4;
    let side = (n + quiet * 2) * module;
    let mut px = vec![0xFFFF_FFFFu32; (side * side) as usize];
    let mut c = Canvas::new(&mut px, side, side);
    for y in 0..n {
        for x in 0..n {
            if code[(x as usize, y as usize)] == qrcode::Color::Dark {
                let r = RectF::new(
                    ((x + quiet) * module) as f32,
                    ((y + quiet) * module) as f32,
                    module as f32,
                    module as f32,
                );
                c.fill_rect(r, Color::BLACK);
            }
        }
    }
    Image {
        w: side,
        h: side,
        rgba: argb_to_rgba(&px),
    }
}

#[test]
fn qr_code_is_decoded() {
    let img = qr_image("https://github.com/Mailo037/krab", 6);
    assert_eq!(
        ocr::read_qr_codes(&img),
        vec!["https://github.com/Mailo037/krab".to_string()]
    );
}

#[test]
fn qr_code_wins_when_there_is_no_text_engine_result() {
    // Whatever the OCR engine makes of a QR code, the payload must be in the result.
    let img = qr_image("krab:hello", 8);
    let text = ocr::extract_text(&img).expect("QR payload is enough for success");
    assert!(text.contains("krab:hello"), "{text:?}");
}

/// Needs an OCR engine (built into Windows; `tesseract` elsewhere), so it is
/// opt-in: `cargo test --test text_actions -- --ignored`.
#[test]
#[ignore]
fn rendered_text_is_recognized() {
    let (w, h) = (520u32, 150u32);
    let mut px = vec![0xFFFF_FFFFu32; (w * h) as usize];
    let mut c = Canvas::new(&mut px, w, h);
    text::ui().draw(
        &mut c,
        "Krab grabs screenshots",
        24.0,
        60.0,
        30.0,
        Color::BLACK,
    );
    text::ui().draw(&mut c, "Invoice 20261112", 24.0, 115.0, 30.0, Color::BLACK);
    let img = Image {
        w,
        h,
        rgba: argb_to_rgba(&px),
    };
    let text = ocr::extract_text(&img).expect("OCR engine available");
    assert!(text.contains("Krab grabs screenshots"), "{text:?}");
    assert!(text.contains("20261112"), "{text:?}");
    assert_eq!(text.lines().count(), 2, "{text:?}");
}
