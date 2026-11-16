/*
*   Copyright (c) 2026. caoccao.com Sam Cao
*   All rights reserved.

*   Licensed under the Apache License, Version 2.0 (the "License");
*   you may not use this file except in compliance with the License.
*   You may obtain a copy of the License at

*   http://www.apache.org/licenses/LICENSE-2.0

*   Unless required by applicable law or agreed to in writing, software
*   distributed under the License is distributed on an "AS IS" BASIS,
*   WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
*   See the License for the specific language governing permissions and
*   limitations under the License.
*/

//! What a failed merge tells the user: each [`MergeError`] in the app's
//! language, with `mkvmerge`'s own error text when it printed one.

use bmm_core::merge::MergeError;

/// The message of a failed merge, translated now.
pub fn merge_error_message(error: &MergeError) -> String {
  match error {
    MergeError::SourceMissing { path } => rust_i18n::t!("merge.error.sourceMissing", path = path).into_owned(),
    MergeError::NotAvailable { path, detail } => {
      rust_i18n::t!("merge.error.mkvmergeNotAvailable", detail = format!("{path}: {detail}")).into_owned()
    }
    MergeError::Exited { code, messages } if messages.is_empty() => match code {
      Some(code) => rust_i18n::t!("merge.error.mkvmergeExitCode", code = code).into_owned(),
      None => rust_i18n::t!("merge.error.mkvmergeTerminated").into_owned(),
    },
    MergeError::Exited { messages, .. } => {
      rust_i18n::t!("merge.error.mkvmergeFailed", detail = messages.join("\n")).into_owned()
    }
    MergeError::Other(message) => message.clone(),
  }
}

/// The text of a completed merge's warnings: at most `limit`, one per line,
/// with `…` when there were more.
pub fn warnings_text(warnings: &[String], limit: usize) -> String {
  let mut lines: Vec<&str> = warnings.iter().take(limit).map(String::as_str).collect();
  if warnings.len() > limit {
    lines.push("…");
  }
  lines.join("\n")
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::i18n::tests::LOCALE_LOCK;
  use bmm_core::config::Language;

  #[test]
  fn every_failure_is_worded_in_the_app_language() {
    let _locale = LOCALE_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    crate::i18n::set_language(Language::EnUS);
    let exited = |code, messages: &[&str]| MergeError::Exited {
      code,
      messages: messages.iter().map(|message| message.to_string()).collect(),
    };
    assert_eq!(
      merge_error_message(&MergeError::SourceMissing {
        path: "/m/a.mkv".into()
      }),
      "The source file /m/a.mkv cannot be found."
    );
    assert_eq!(
      merge_error_message(&MergeError::NotAvailable {
        path: "/usr/bin/mkvmerge".into(),
        detail: "No such file or directory (os error 2)".into(),
      }),
      "mkvmerge is not available. Please check the MKVToolNix path in Settings. \
       /usr/bin/mkvmerge: No such file or directory (os error 2)"
    );
    assert_eq!(
      merge_error_message(&exited(Some(2), &["The file 'x' could not be opened for writing."])),
      "mkvmerge failed: The file 'x' could not be opened for writing."
    );
    assert_eq!(
      merge_error_message(&exited(Some(2), &[])),
      "mkvmerge exited with code 2."
    );
    assert_eq!(merge_error_message(&exited(None, &[])), "mkvmerge was terminated.");
    assert_eq!(merge_error_message(&MergeError::Other("as is".into())), "as is");
    crate::i18n::set_language(Language::De);
    assert_eq!(
      merge_error_message(&exited(None, &[])),
      "mkvmerge wurde vorzeitig beendet."
    );
    crate::i18n::set_language(Language::EnUS);
  }

  #[test]
  fn warnings_are_listed_up_to_the_limit() {
    let warnings: Vec<String> = ["one", "two", "three"].map(String::from).to_vec();
    assert_eq!(warnings_text(&warnings, 3), "one\ntwo\nthree");
    assert_eq!(warnings_text(&warnings, 2), "one\ntwo\n…");
    assert_eq!(warnings_text(&[], 2), "");
  }
}
