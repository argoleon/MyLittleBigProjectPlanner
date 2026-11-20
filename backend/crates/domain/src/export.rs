//! Exports of list views (specification §6, L06-US06): the rows of a list
//! with its displayed columns, as a file other tools read.
//!
//! - CSV: UTF-8 with a byte order mark (spreadsheets then read accents
//!   right), comma-separated, CRLF line ends, fields quoted when they hold a
//!   comma, a quote or a line break (RFC 4180).
//! - Values are unambiguous whatever the reader's locale: dates as
//!   `YYYY-MM-DD`, times as RFC 3339 in UTC, numbers with a dot and no
//!   thousands separator.
//! - A text starting with `=`, `+`, `-`, `@`, a tab or a carriage return is
//!   prefixed with `'`: a spreadsheet would run it as a formula (formula
//!   injection, OWASP). Numbers and dates are never text, so `-3` stays a
//!   number.
//! - Excel (L06-US07): typed cells, so that spreadsheets sort and compute
//!   them: numbers as numbers, dates and times as dates (times in UTC, said
//!   in their header), texts as texts (never run as formulas).
//! - Up to [`SYNC_MAX_ROWS`] rows are downloaded at once; beyond, the
//!   worker writes the file, kept [`FILE_LIFETIME_HOURS`] hours; never more
//!   than [`MAX_ROWS`] rows.

/// Most rows of an export downloaded at once; beyond, it runs in the background.
pub const SYNC_MAX_ROWS: usize = 1_000;
/// Most rows of any export: beyond, the member narrows the query.
pub const MAX_ROWS: usize = 100_000;
/// How long a background export's file is kept.
pub const FILE_LIFETIME_HOURS: i64 = 24;
/// Most columns of an export.
pub const MAX_COLUMNS: usize = 40;

/// The format of an export file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Format {
    #[default]
    Csv,
    Xlsx,
}

impl Format {
    pub const ALL: [Self; 2] = [Self::Csv, Self::Xlsx];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Xlsx => "xlsx",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.as_str() == raw)
    }

    /// The file's media type.
    pub const fn content_type(self) -> &'static str {
        match self {
            Self::Csv => "text/csv; charset=utf-8",
            Self::Xlsx => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        }
    }

    /// The file name extension.
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Xlsx => "xlsx",
        }
    }
}

/// A column of a ticket list, by the id the screens use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    Key,
    Summary,
    Project,
    Type,
    Status,
    Priority,
    Assignee,
    Reporter,
    Due,
    Start,
    Created,
    Updated,
}

impl Column {
    pub const ALL: [Self; 12] = [
        Self::Key,
        Self::Summary,
        Self::Project,
        Self::Type,
        Self::Status,
        Self::Priority,
        Self::Assignee,
        Self::Reporter,
        Self::Due,
        Self::Start,
        Self::Created,
        Self::Updated,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Key => "key",
            Self::Summary => "summary",
            Self::Project => "project",
            Self::Type => "type",
            Self::Status => "status",
            Self::Priority => "priority",
            Self::Assignee => "assignee",
            Self::Reporter => "reporter",
            Self::Due => "due",
            Self::Start => "start",
            Self::Created => "created",
            Self::Updated => "updated",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.as_str() == raw)
    }

    /// Its header, as the screens name it, in `language` (`fr`, else English).
    pub const fn header(self, language: &str) -> &'static str {
        let fr = matches!(language.as_bytes(), b"fr");
        match (self, fr) {
            (Self::Key, true) => "Clé",
            (Self::Key, false) => "Key",
            (Self::Summary, true) => "Titre",
            (Self::Summary, false) => "Title",
            (Self::Project, true) => "Projet",
            (Self::Project, false) => "Project",
            (Self::Type, _) => "Type",
            (Self::Status, true) => "Statut",
            (Self::Status, false) => "Status",
            (Self::Priority, true) => "Priorité",
            (Self::Priority, false) => "Priority",
            (Self::Assignee, true) => "Responsable",
            (Self::Assignee, false) => "Assignee",
            (Self::Reporter, true) => "Rapporteur",
            (Self::Reporter, false) => "Reporter",
            (Self::Due, true) => "Échéance",
            (Self::Due, false) => "Due date",
            (Self::Start, true) => "Début",
            (Self::Start, false) => "Start date",
            (Self::Created, true) => "Créé le",
            (Self::Created, false) => "Created",
            (Self::Updated, true) => "Modifié le",
            (Self::Updated, false) => "Updated",
        }
    }
}

impl Column {
    /// Its header in a file of `format`: an Excel time has no time zone, so
    /// the header of a time column tells it is in UTC.
    pub fn header_in(self, language: &str, format: Format) -> String {
        let header = self.header(language);
        if format == Format::Xlsx && matches!(self, Self::Created | Self::Updated) {
            format!("{header} (UTC)")
        } else {
            header.to_owned()
        }
    }
}

/// The columns asked for, in their order, each once; `Err` with the
/// position of the first unknown one, or when there are none or too many.
pub fn columns(ids: &[String]) -> Result<Vec<Column>, crate::validation::FieldError> {
    if ids.is_empty() {
        return Err(crate::validation::FieldError::new("columns", "required"));
    }
    if ids.len() > MAX_COLUMNS {
        return Err(crate::validation::FieldError::new("columns", "too_many"));
    }
    let mut out = Vec::with_capacity(ids.len());
    for (i, id) in ids.iter().enumerate() {
        let column = Column::parse(id)
            .ok_or_else(|| crate::validation::FieldError::new(format!("columns.{i}"), "unknown_field"))?;
        if out.contains(&column) {
            return Err(crate::validation::FieldError::new(
                format!("columns.{i}"),
                "duplicate",
            ));
        }
        out.push(column);
    }
    Ok(out)
}

/// The name of a priority (`crate::issues::Priority` ids) in `language`.
pub const fn priority_label(priority: &str, language: &str) -> Option<&'static str> {
    let fr = matches!(language.as_bytes(), b"fr");
    Some(match (priority.as_bytes(), fr) {
        (b"highest", true) => "Très haute",
        (b"highest", false) => "Highest",
        (b"high", true) => "Haute",
        (b"high", false) => "High",
        (b"medium", true) => "Moyenne",
        (b"medium", false) => "Medium",
        (b"low", true) => "Basse",
        (b"low", false) => "Low",
        (b"lowest", true) => "Très basse",
        (b"lowest", false) => "Lowest",
        _ => return None,
    })
}

/// A cell of an exported row. Dates and times come already written in
/// their unambiguous form (this crate reads no clock nor calendar).
#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    Empty,
    Text(String),
    /// `YYYY-MM-DD`.
    Date(String),
    /// RFC 3339, in UTC.
    DateTime(String),
    Number(f64),
}

/// The first characters that make a spreadsheet read a text as a formula.
const FORMULA_STARTS: [char; 6] = ['=', '+', '-', '@', '\t', '\r'];

/// `text` as a spreadsheet must not run it: prefixed with `'` when it
/// starts like a formula.
pub fn neutralized(text: &str) -> std::borrow::Cow<'_, str> {
    if text.starts_with(FORMULA_STARTS) {
        std::borrow::Cow::Owned(format!("'{text}"))
    } else {
        std::borrow::Cow::Borrowed(text)
    }
}

/// A number with a dot, no exponent, no thousands separator; integers
/// without decimals. Not a finite number: empty.
pub fn number(value: f64) -> String {
    if !value.is_finite() {
        return String::new();
    }
    if value.fract() == 0.0 && value.abs() < 1e15 {
        // Whole: no ".0".
        #[allow(clippy::cast_possible_truncation)]
        return format!("{}", value as i64);
    }
    format!("{value}")
}

/// One CSV field: neutralized when text, quoted when needed.
fn field(cell: &Cell, out: &mut String) {
    let raw: std::borrow::Cow<'_, str> = match cell {
        Cell::Empty => return,
        Cell::Text(text) => neutralized(text),
        Cell::Date(value) | Cell::DateTime(value) => std::borrow::Cow::Borrowed(value.as_str()),
        Cell::Number(value) => std::borrow::Cow::Owned(number(*value)),
    };
    if raw.contains([',', '"', '\n', '\r']) {
        out.push('"');
        out.push_str(&raw.replace('"', "\"\""));
        out.push('"');
    } else {
        out.push_str(&raw);
    }
}

/// One CSV line, CRLF included.
pub fn csv_line(cells: &[Cell], out: &mut String) {
    for (i, cell) in cells.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        field(cell, out);
    }
    out.push_str("\r\n");
}

/// The start of a CSV file: the byte order mark and the header line.
pub fn csv_header(headers: &[String]) -> String {
    let mut out = String::from('\u{feff}');
    let cells: Vec<Cell> = headers.iter().map(|h| Cell::Text(h.clone())).collect();
    csv_line(&cells, &mut out);
    out
}

/// A whole CSV file.
pub fn csv(headers: &[String], rows: &[Vec<Cell>]) -> String {
    let mut out = csv_header(headers);
    for row in rows {
        csv_line(row, &mut out);
    }
    out
}

/// The name of an export file: `prefix-YYYY-MM-DD.ext`, the prefix reduced
/// to letters, digits and dashes.
pub fn file_name(prefix: &str, date: &str, format: Format) -> String {
    let clean: String = prefix
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let clean = clean
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let clean = if clean.is_empty() {
        "export".to_owned()
    } else {
        clean
    };
    format!("{clean}-{date}.{}", format.extension())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> Cell {
        Cell::Text(value.to_owned())
    }

    #[test]
    fn a_file_starts_with_the_byte_order_mark_and_the_headers() {
        let file = csv(
            &["Key".to_owned(), "Title".to_owned()],
            &[vec![text("PLAT-1"), text("Migrate to OIDC")]],
        );
        assert!(file.starts_with('\u{feff}'));
        assert_eq!(file.as_bytes()[..3], [0xEF, 0xBB, 0xBF]);
        assert_eq!(file, "\u{feff}Key,Title\r\nPLAT-1,Migrate to OIDC\r\n");
    }

    #[test]
    fn fields_with_commas_quotes_or_line_breaks_are_quoted() {
        let mut out = String::new();
        csv_line(
            &[
                text("a, b"),
                text("say \"hi\""),
                text("two\nlines"),
                text("cr\rhere"),
                text("plain"),
                Cell::Empty,
            ],
            &mut out,
        );
        assert_eq!(
            out,
            "\"a, b\",\"say \"\"hi\"\"\",\"two\nlines\",\"cr\rhere\",plain,\r\n"
        );
    }

    #[test]
    fn texts_starting_like_a_formula_are_neutralized() {
        for start in ["=SUM(A1)", "+33 6", "-2+3", "@cmd", "\tTab", "\rCR"] {
            assert_eq!(neutralized(start), format!("'{start}"), "{start:?}");
        }
        assert_eq!(neutralized("Plain = text"), "Plain = text");
        assert_eq!(neutralized(""), "");
        let mut out = String::new();
        csv_line(&[text("=HYPERLINK(\"http://x\",\"y\")"), text("-1")], &mut out);
        assert_eq!(out, "\"'=HYPERLINK(\"\"http://x\"\",\"\"y\"\")\",'-1\r\n");
    }

    #[test]
    fn numbers_and_dates_are_written_unambiguously() {
        let mut out = String::new();
        csv_line(
            &[
                Cell::Number(-3.0),
                Cell::Number(2.5),
                Cell::Number(1234567.25),
                Cell::Number(0.1),
                Cell::Number(f64::NAN),
                Cell::Date("2026-11-20".to_owned()),
                Cell::DateTime("2026-11-18T09:12:00Z".to_owned()),
            ],
            &mut out,
        );
        assert_eq!(out, "-3,2.5,1234567.25,0.1,,2026-11-20,2026-11-18T09:12:00Z\r\n");
        assert_eq!(number(5.0), "5");
        assert_eq!(number(-0.0), "0");
        assert_eq!(number(1e20), "100000000000000000000");
        assert_eq!(number(f64::INFINITY), "");
    }

    #[test]
    fn headers_are_neutralized_too() {
        assert_eq!(csv_header(&["=Key".to_owned()]), "\u{feff}'=Key\r\n");
    }

    #[test]
    fn accents_stay_as_written() {
        assert_eq!(
            csv(&["Résumé".to_owned()], &[vec![text("Créer l’annuaire")]]),
            "\u{feff}Résumé\r\nCréer l’annuaire\r\n"
        );
    }

    #[test]
    fn file_names_are_safe() {
        assert_eq!(
            file_name("Search", "2026-11-18", Format::Csv),
            "search-2026-11-18.csv"
        );
        assert_eq!(
            file_name("My tickets!", "2026-11-18", Format::Csv),
            "my-tickets-2026-11-18.csv"
        );
        assert_eq!(
            file_name("Écran/..", "2026-11-18", Format::Csv),
            "cran-2026-11-18.csv"
        );
        assert_eq!(file_name("", "2026-11-18", Format::Csv), "export-2026-11-18.csv");
    }

    #[test]
    fn formats_round_trip() {
        for format in Format::ALL {
            assert_eq!(Format::parse(format.as_str()), Some(format));
        }
        assert_eq!(Format::parse("pdf"), None);
        assert_eq!(Format::Csv.content_type(), "text/csv; charset=utf-8");
        assert_eq!(
            Format::Xlsx.content_type(),
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        );
        assert_eq!(
            file_name("Search", "2026-11-18", Format::Xlsx),
            "search-2026-11-18.xlsx"
        );
    }

    #[test]
    fn columns_are_read_in_order_once_each() {
        let ids = |list: &[&str]| list.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            columns(&ids(&["key", "summary", "status", "assignee"])).unwrap(),
            [Column::Key, Column::Summary, Column::Status, Column::Assignee]
        );
        let code = |list: &[&str]| {
            let e = columns(&ids(list)).unwrap_err();
            (e.field, e.code)
        };
        assert_eq!(code(&[]), ("columns".to_owned(), "required"));
        assert_eq!(
            code(&["key", "points"]),
            ("columns.1".to_owned(), "unknown_field")
        );
        assert_eq!(
            code(&["key", "summary", "key"]),
            ("columns.2".to_owned(), "duplicate")
        );
        let many: Vec<&str> = std::iter::repeat_n("key", MAX_COLUMNS + 1).collect();
        assert_eq!(code(&many), ("columns".to_owned(), "too_many"));
        for column in Column::ALL {
            assert_eq!(Column::parse(column.as_str()), Some(column));
        }
    }

    #[test]
    fn headers_and_priorities_follow_the_language() {
        let headers = |language: &str| {
            [Column::Key, Column::Summary, Column::Status, Column::Assignee].map(|c| c.header(language))
        };
        assert_eq!(headers("en"), ["Key", "Title", "Status", "Assignee"]);
        assert_eq!(headers("fr"), ["Clé", "Titre", "Statut", "Responsable"]);
        assert_eq!(headers("de"), headers("en"));
        assert_eq!(priority_label("highest", "fr"), Some("Très haute"));
        assert_eq!(priority_label("low", "en"), Some("Low"));
        assert_eq!(priority_label("urgent", "en"), None);
    }

    #[test]
    fn excel_time_headers_say_utc() {
        assert_eq!(Column::Created.header_in("en", Format::Xlsx), "Created (UTC)");
        assert_eq!(Column::Updated.header_in("fr", Format::Xlsx), "Modifié le (UTC)");
        assert_eq!(Column::Created.header_in("en", Format::Csv), "Created");
        assert_eq!(
            Column::Due.header_in("en", Format::Xlsx),
            "Due date",
            "a planning date has no zone"
        );
    }
}
