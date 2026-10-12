//! Text recognition in images with the Vision framework, for Copy Text.

use std::path::Path;

use objc2::AnyThread;
use objc2_foundation::{NSArray, NSDictionary, NSURL};
use objc2_vision::{
    VNImageRequestHandler, VNRecognizeTextRequest, VNRequest, VNRequestTextRecognitionLevel,
};

/// The text in the images, line by line; images are separated by a blank
/// line. Safe to call off the main thread. Empty if there is no text.
pub fn recognize_text(paths: &[impl AsRef<Path>]) -> Result<String, String> {
    let mut texts = Vec::new();
    for path in paths {
        let text = recognize(path.as_ref())?;
        if !text.is_empty() {
            texts.push(text);
        }
    }
    Ok(texts.join("\n\n"))
}

fn recognize(path: &Path) -> Result<String, String> {
    let url = NSURL::from_file_path(path)
        .ok_or_else(|| format!("not a file path: {}", path.display()))?;
    // SAFETY: A file URL and an empty options dictionary.
    let handler = unsafe {
        VNImageRequestHandler::initWithURL_options(
            VNImageRequestHandler::alloc(),
            &url,
            &NSDictionary::new(),
        )
    };
    let request = VNRecognizeTextRequest::new();
    request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
    request.setUsesLanguageCorrection(true);
    request.setAutomaticallyDetectsLanguage(true);
    let requests: &VNRequest = &request;
    handler
        .performRequests_error(&NSArray::from_slice(&[requests]))
        .map_err(|error| error.localizedDescription().to_string())?;
    let lines: Vec<String> = request
        .results()
        .map(|observations| {
            observations
                .iter()
                .filter_map(|observation| {
                    observation
                        .topCandidates(1)
                        .firstObject()
                        .map(|text| text.string().to_string())
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(lines.join("\n"))
}
