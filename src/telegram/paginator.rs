#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Paginator {
    page: usize,
    pages: usize,
    start: usize,
    end: usize,
}

impl Paginator {
    pub fn new(total: usize, requested_page: usize, page_size: usize) -> Self {
        let page_size = page_size.max(1);
        let pages = total.div_ceil(page_size).max(1);
        let page = requested_page.clamp(1, pages);
        let start = (page - 1) * page_size;
        Self {
            page,
            pages,
            start,
            end: (start + page_size).min(total),
        }
    }

    pub fn page(self) -> usize {
        self.page
    }
    pub fn pages(self) -> usize {
        self.pages
    }
    pub fn range(self) -> std::ops::Range<usize> {
        self.start..self.end
    }
    pub fn previous(self) -> usize {
        self.page.saturating_sub(1).max(1)
    }
    pub fn next(self) -> usize {
        (self.page + 1).min(self.pages)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_pages_and_never_returns_more_than_five() {
        let page = Paginator::new(12, 2, 5);
        assert_eq!(page.range(), 5..10);
        assert_eq!((page.page(), page.pages()), (2, 3));
        assert_eq!(Paginator::new(0, 99, 5).range(), 0..0);
    }
}
