use std::cmp::Ordering;

use ratatui::widgets::ListState;

use super::fuzzy::fuzzy_text_score_lower;

const MAX_RESULTS: usize = 20;

#[derive(Debug, Clone)]
struct IndexedFile {
    path_lower: String,
    name_lower: String,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct FileSearchResult {
    pub(crate) file_index: usize,
    score: u32,
}

#[derive(Debug, Default)]
pub(crate) struct FileSearch {
    pub(crate) query: String,
    pub(crate) results: Vec<FileSearchResult>,
    pub(crate) state: ListState,
    pub(crate) match_count: usize,
    index: Vec<IndexedFile>,
    files_fingerprint: Option<u64>,
}

impl FileSearch {
    pub(crate) fn new(files: &[String], files_fingerprint: Option<u64>) -> Self {
        let mut search = Self::default();
        search.reindex(files, files_fingerprint);
        search
    }

    pub(crate) fn reindex(&mut self, files: &[String], files_fingerprint: Option<u64>) {
        if self.files_fingerprint == files_fingerprint {
            return;
        }
        let _activity =
            crate::diagnostics::activity("index-file-search", format!("files={}", files.len()));
        let index = files
            .iter()
            .map(|path| {
                let path_lower = path.to_lowercase();
                let name_lower = path_lower
                    .rsplit('/')
                    .next()
                    .unwrap_or(&path_lower)
                    .to_owned();
                IndexedFile {
                    path_lower,
                    name_lower,
                }
            })
            .collect();
        let previous = std::mem::replace(&mut self.index, index);
        if previous.len() >= 10_000 {
            crate::diagnostics::drop_in_background("file-search-index", previous);
        }
        self.files_fingerprint = files_fingerprint;
        self.refresh(files);
    }

    pub(crate) fn invalidate(&mut self) {
        let previous = std::mem::take(&mut self.index);
        if previous.len() >= 10_000 {
            crate::diagnostics::drop_in_background("file-search-index", previous);
        }
        self.files_fingerprint = None;
        self.open();
    }

    pub(crate) fn open(&mut self) {
        self.query.clear();
        self.results.clear();
        self.match_count = 0;
        self.state = ListState::default();
    }

    pub(crate) fn push(&mut self, character: char, files: &[String]) {
        self.query.push(character);
        self.refresh(files);
    }

    pub(crate) fn paste(&mut self, text: &str, files: &[String]) {
        self.query.extend(
            text.chars()
                .filter(|character| !matches!(character, '\r' | '\n')),
        );
        self.refresh(files);
    }

    pub(crate) fn backspace(&mut self, files: &[String]) {
        self.query.pop();
        self.refresh(files);
    }

    pub(crate) fn clear(&mut self, files: &[String]) {
        self.query.clear();
        self.refresh(files);
    }

    pub(crate) fn move_selection(&mut self, delta: isize) {
        if self.results.is_empty() {
            self.state.select(None);
            return;
        }
        let current = self.state.selected().unwrap_or(0);
        self.state.select(Some(
            current
                .saturating_add_signed(delta)
                .min(self.results.len() - 1),
        ));
    }

    pub(crate) fn select(&mut self, index: usize) -> bool {
        if index >= self.results.len() {
            return false;
        }
        self.state.select(Some(index));
        true
    }

    pub(crate) fn selected_file_index(&self) -> Option<usize> {
        self.state
            .selected()
            .and_then(|index| self.results.get(index))
            .map(|result| result.file_index)
    }

    fn refresh(&mut self, files: &[String]) {
        self.results.clear();
        self.match_count = 0;
        let query = self.query.trim().to_lowercase();
        let terms: Vec<_> = query.split_whitespace().collect();
        if terms.is_empty() {
            self.state.select(None);
            return;
        }

        for (file_index, file) in self.index.iter().enumerate() {
            let Some(score) = file_score(&terms, file) else {
                continue;
            };
            self.match_count += 1;
            let candidate = FileSearchResult { file_index, score };
            if self.results.len() < MAX_RESULTS {
                self.results.push(candidate);
            } else if let Some((worst, _)) = self
                .results
                .iter()
                .enumerate()
                .max_by(|(_, left), (_, right)| result_order(**left, **right, files))
                && result_order(candidate, self.results[worst], files).is_lt()
            {
                self.results[worst] = candidate;
            }
        }
        self.results
            .sort_by(|left, right| result_order(*left, *right, files));
        self.state.select((!self.results.is_empty()).then_some(0));
    }
}

fn file_score(terms: &[&str], file: &IndexedFile) -> Option<u32> {
    terms.iter().try_fold(0_u32, |total, term| {
        let path_score = fuzzy_text_score_lower(term, &file.path_lower);
        let name_score =
            fuzzy_text_score_lower(term, &file.name_lower).map(|score| score.saturating_add(1_500));
        path_score
            .into_iter()
            .chain(name_score)
            .max()
            .map(|score| total.saturating_add(score))
    })
}

fn result_order(left: FileSearchResult, right: FileSearchResult, files: &[String]) -> Ordering {
    right
        .score
        .cmp(&left.score)
        .then_with(|| files.get(left.file_index).cmp(&files.get(right.file_index)))
}

#[cfg(test)]
mod tests {
    use super::{FileSearch, MAX_RESULTS};

    #[test]
    fn favors_basenames_and_matches_multiple_terms() {
        let files = vec![
            "src/application.rs".to_owned(),
            "docs/app-notes.md".to_owned(),
            "src/ui/app_view.rs".to_owned(),
        ];
        let mut search = FileSearch::new(&files, Some(1));

        for character in "app view".chars() {
            search.push(character, &files);
        }

        assert_eq!(search.match_count, 1);
        assert_eq!(search.selected_file_index(), Some(2));
    }

    #[test]
    fn keeps_only_the_best_results() {
        let files = (0..100)
            .map(|index| format!("src/file-{index:03}.rs"))
            .collect::<Vec<_>>();
        let mut search = FileSearch::new(&files, Some(1));

        search.push('f', &files);

        assert_eq!(search.results.len(), MAX_RESULTS);
        assert_eq!(search.match_count, files.len());
    }
}
