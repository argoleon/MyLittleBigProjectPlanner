//! Excel files of list view exports (L06-US07): one sheet, its headers in
//! bold and frozen with a filter, each cell typed so that spreadsheets sort
//! and compute it: numbers as numbers, dates as dates (`yyyy-mm-dd`), times
//! as date-times in UTC (`yyyy-mm-dd hh:mm:ss`), texts as texts, which
//! Excel never runs as formulas.

use domain::export::Cell;
use rust_xlsxwriter::{ExcelDateTime, Format, Workbook, XlsxError};
use time::format_description::well_known::Rfc3339;
use time::{Date, OffsetDateTime, UtcOffset};

/// Widest a column is made, in characters.
const MAX_WIDTH: usize = 60;

/// The workbook of `headers` and `rows`, as the bytes of an `.xlsx` file.
pub fn write(headers: &[String], rows: &[Vec<Cell>]) -> Result<Vec<u8>, XlsxError> {
    let mut workbook = Workbook::new();
    let sheet = workbook.add_worksheet();
    let bold = Format::new().set_bold();
    let date = Format::new().set_num_format("yyyy-mm-dd");
    let date_time = Format::new().set_num_format("yyyy-mm-dd hh:mm:ss");
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for (column, header) in headers.iter().enumerate() {
        sheet.write_string_with_format(0, index(column), header, &bold)?;
    }
    for (r, row) in rows.iter().enumerate() {
        let r = u32::try_from(r + 1).unwrap_or(u32::MAX);
        for (column, cell) in row.iter().enumerate() {
            let c = index(column);
            let width = match cell {
                Cell::Text(text) => {
                    sheet.write_string(r, c, text)?;
                    text.chars().count()
                }
                Cell::Number(value) if value.is_finite() => {
                    sheet.write_number(r, c, *value)?;
                    domain::export::number(*value).len()
                }
                // Nothing to write: no value, or not a finite number.
                Cell::Empty | Cell::Number(_) => 0,
                Cell::Date(value) => match excel_date(value) {
                    Some(at) => {
                        sheet.write_datetime_with_format(r, c, &at, &date)?;
                        10
                    }
                    None => {
                        sheet.write_string(r, c, value)?;
                        value.len()
                    }
                },
                Cell::DateTime(value) => match excel_date_time(value) {
                    Some(at) => {
                        sheet.write_datetime_with_format(r, c, &at, &date_time)?;
                        19
                    }
                    None => {
                        sheet.write_string(r, c, value)?;
                        value.len()
                    }
                },
            };
            if let Some(widest) = widths.get_mut(column) {
                *widest = (*widest).max(width);
            }
        }
    }
    for (column, width) in widths.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        sheet.set_column_width(index(column), ((*width).min(MAX_WIDTH) + 2) as f64)?;
    }
    if !headers.is_empty() {
        sheet.set_freeze_panes(1, 0)?;
        let last_row = u32::try_from(rows.len()).unwrap_or(u32::MAX);
        sheet.autofilter(0, 0, last_row, index(headers.len() - 1))?;
    }
    workbook.save_to_buffer()
}

fn index(column: usize) -> u16 {
    u16::try_from(column).unwrap_or(u16::MAX)
}

/// `YYYY-MM-DD` as an Excel date.
fn excel_date(value: &str) -> Option<ExcelDateTime> {
    let date = Date::parse(value, &time::format_description::well_known::Iso8601::DATE).ok()?;
    ExcelDateTime::from_ymd(u16::try_from(date.year()).ok()?, date.month().into(), date.day()).ok()
}

/// An RFC 3339 time as an Excel date-time, in UTC.
fn excel_date_time(value: &str) -> Option<ExcelDateTime> {
    let at = OffsetDateTime::parse(value, &Rfc3339)
        .ok()?
        .to_offset(UtcOffset::UTC);
    ExcelDateTime::from_ymd(u16::try_from(at.year()).ok()?, at.month().into(), at.day())
        .ok()?
        .and_hms(u16::from(at.hour()), at.minute(), at.second())
        .ok()
}

#[cfg(test)]
mod tests {
    use calamine::{Data, Reader, Xlsx};

    use super::*;

    fn read(bytes: Vec<u8>) -> Vec<Vec<Data>> {
        let mut book: Xlsx<_> = calamine::open_workbook_from_rs(std::io::Cursor::new(bytes)).unwrap();
        let range = book.worksheet_range_at(0).unwrap().unwrap();
        range.rows().map(<[Data]>::to_vec).collect()
    }

    #[test]
    fn cells_are_typed_numbers_dates_and_texts() {
        let headers = ["Key", "Points", "Due date", "Created (UTC)", "Note"].map(str::to_owned);
        let rows = vec![
            vec![
                Cell::Text("PLAT-1".to_owned()),
                Cell::Number(2.5),
                Cell::Date("2026-11-20".to_owned()),
                Cell::DateTime("2026-11-18T09:12:30+02:00".to_owned()),
                Cell::Text("=SUM(A1)".to_owned()),
            ],
            vec![
                Cell::Text("PLAT-2".to_owned()),
                Cell::Number(-3.0),
                Cell::Empty,
                Cell::Empty,
                Cell::Empty,
            ],
        ];
        let sheet = read(write(&headers, &rows).unwrap());
        assert_eq!(
            sheet[0],
            ["Key", "Points", "Due date", "Created (UTC)", "Note"].map(|h| Data::String(h.to_owned()))
        );
        assert_eq!(sheet[1][0], Data::String("PLAT-1".to_owned()));
        assert_eq!(sheet[1][1], Data::Float(2.5));
        // A text, never a formula.
        assert_eq!(sheet[1][4], Data::String("=SUM(A1)".to_owned()));
        let Data::DateTime(due) = &sheet[1][2] else {
            panic!("a date, not {:?}", sheet[1][2]);
        };
        // Excel serials: days since 1899-12-30.
        assert!(due.is_datetime());
        assert!(
            (due.as_f64() - 46_346.0).abs() < 1e-9,
            "2026-11-20: {}",
            due.as_f64()
        );
        let Data::DateTime(created) = &sheet[1][3] else {
            panic!("a date-time, not {:?}", sheet[1][3]);
        };
        // 2026-11-18 07:12:30 UTC.
        assert!(created.is_datetime());
        let expected = 46_344.0 + (7.0 * 3600.0 + 12.0 * 60.0 + 30.0) / 86_400.0;
        assert!((created.as_f64() - expected).abs() < 1e-6, "{}", created.as_f64());
        assert_eq!(sheet[2][1], Data::Float(-3.0));
        assert_eq!(sheet[2][2], Data::Empty);
    }

    #[test]
    fn unreadable_dates_stay_as_written_and_odd_numbers_are_left_out() {
        let rows = vec![vec![
            Cell::Date("someday".to_owned()),
            Cell::DateTime("later".to_owned()),
            Cell::Number(f64::NAN),
        ]];
        let sheet = read(write(&["A", "B", "C"].map(str::to_owned), &rows).unwrap());
        assert_eq!(sheet[1][0], Data::String("someday".to_owned()));
        assert_eq!(sheet[1][1], Data::String("later".to_owned()));
        assert_eq!(sheet[1].get(2).cloned().unwrap_or(Data::Empty), Data::Empty);
    }

    #[test]
    fn an_empty_list_keeps_its_headers() {
        let sheet = read(write(&["Key".to_owned()], &[]).unwrap());
        assert_eq!(sheet, vec![vec![Data::String("Key".to_owned())]]);
    }
}
