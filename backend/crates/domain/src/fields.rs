//! Fields of a project configuration (specification §5, L04-US03): system
//! and custom fields, their options, the values a ticket may hold, and
//! whether each is hidden, visible or required per issue type and workflow
//! step.
//!
//! A field applies to the types it lists, or to all of them. For a type it
//! applies to, a step nobody set is Visible, statuses added later included:
//! only the other cells are stored. A type it does not apply to never shows
//! it. The values of a list field keep their identity; once published, a
//! value is deactivated rather than removed, so that tickets keep it.
//!
//! Tickets (L05) and portal forms (L13) check their values with
//! [`validate_value`] and their required fields with [`mode`].

use std::collections::{BTreeMap, BTreeSet};

use crate::i18n;
use crate::issue_types::is_key;
use crate::validation::{FieldError, Violations};

/// Longest label of a field or of a list value, in characters, per language.
pub const FIELD_LABEL_MAX_CHARS: usize = 80;
/// Most values of a list field, its children included.
pub const MAX_VALUES: usize = 200;
/// Longest short text, and the default limit of a short text field.
pub const SHORT_TEXT_MAX_CHARS: u32 = 255;
/// Longest rich long text, in characters.
pub const RICH_TEXT_MAX_CHARS: usize = 100_000;
/// Most decimals of a number field.
pub const MAX_DECIMALS: u8 = 4;
/// Longest label of a labels field, in characters; no spaces.
pub const LABEL_MAX_CHARS: usize = 50;
/// Most labels on a ticket.
pub const MAX_LABELS: usize = 30;
/// Longest URL, in characters.
pub const URL_MAX_CHARS: usize = 2048;

/// The type of a field (the MVP ones of specification §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    ShortText,
    RichText,
    Number,
    Checkbox,
    Date,
    DateTime,
    SingleSelect,
    MultiSelect,
    CascadingSelect,
    User,
    Group,
    Labels,
    Url,
    IssueLink,
}

impl Kind {
    pub const ALL: [Self; 14] = [
        Self::ShortText,
        Self::RichText,
        Self::Number,
        Self::Checkbox,
        Self::Date,
        Self::DateTime,
        Self::SingleSelect,
        Self::MultiSelect,
        Self::CascadingSelect,
        Self::User,
        Self::Group,
        Self::Labels,
        Self::Url,
        Self::IssueLink,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ShortText => "short_text",
            Self::RichText => "rich_text",
            Self::Number => "number",
            Self::Checkbox => "checkbox",
            Self::Date => "date",
            Self::DateTime => "date_time",
            Self::SingleSelect => "single_select",
            Self::MultiSelect => "multi_select",
            Self::CascadingSelect => "cascading_select",
            Self::User => "user",
            Self::Group => "group",
            Self::Labels => "labels",
            Self::Url => "url",
            Self::IssueLink => "issue_link",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == raw)
    }

    /// Whether its values come from a list it defines.
    pub const fn has_values(self) -> bool {
        matches!(
            self,
            Self::SingleSelect | Self::MultiSelect | Self::CascadingSelect
        )
    }

    /// The options it accepts.
    pub const fn accepts(self, option: OptionName) -> bool {
        match option {
            OptionName::MaxLength => matches!(self, Self::ShortText),
            OptionName::Decimals | OptionName::Min | OptionName::Max | OptionName::Unit => {
                matches!(self, Self::Number)
            }
            OptionName::Multiple => matches!(self, Self::User),
            OptionName::Values => self.has_values(),
        }
    }
}

/// The unit of a number field, shown next to its values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Hours,
    Days,
    Percent,
}

impl Unit {
    pub const ALL: [Self; 3] = [Self::Hours, Self::Days, Self::Percent];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hours => "hours",
            Self::Days => "days",
            Self::Percent => "percent",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|unit| unit.as_str() == raw)
    }
}

/// The name of an option, as the API spells it under `options`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionName {
    MaxLength,
    Decimals,
    Min,
    Max,
    Unit,
    Multiple,
    Values,
}

impl OptionName {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MaxLength => "maxLength",
            Self::Decimals => "decimals",
            Self::Min => "min",
            Self::Max => "max",
            Self::Unit => "unit",
            Self::Multiple => "multiple",
            Self::Values => "values",
        }
    }
}

/// A value of a list field: its identity, which it keeps for good, its
/// labels, whether it may still be picked, and for a cascading select the
/// values under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListValue {
    pub id: u128,
    pub labels: BTreeMap<String, String>,
    pub active: bool,
    pub children: Vec<ListValue>,
}

/// The options of a field; each applies to some types only
/// ([`Kind::accepts`]), the others stay `None`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Options {
    /// Short text: longest text, in characters (255 by default).
    pub max_length: Option<u32>,
    /// Number: decimals allowed (none by default).
    pub decimals: Option<u8>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub unit: Option<Unit>,
    /// User: several users (one by default).
    pub multiple: Option<bool>,
    /// Single, multi and cascading select: the values to pick from.
    pub values: Option<Vec<ListValue>>,
}

impl Options {
    fn set(&self) -> Vec<OptionName> {
        let mut set = Vec::new();
        let names = [
            (self.max_length.is_some(), OptionName::MaxLength),
            (self.decimals.is_some(), OptionName::Decimals),
            (self.min.is_some(), OptionName::Min),
            (self.max.is_some(), OptionName::Max),
            (self.unit.is_some(), OptionName::Unit),
            (self.multiple.is_some(), OptionName::Multiple),
            (self.values.is_some(), OptionName::Values),
        ];
        for (is_set, name) in names {
            if is_set {
                set.push(name);
            }
        }
        set
    }

    /// Every value, children included.
    pub fn all_values(&self) -> Vec<&ListValue> {
        let mut all = Vec::new();
        for value in self.values.iter().flatten() {
            all.push(value);
            all.extend(value.children.iter());
        }
        all
    }

    /// The value `id`, active or not, wherever it sits: deactivated values
    /// stay readable on the tickets that hold them.
    pub fn value(&self, id: u128) -> Option<&ListValue> {
        self.all_values().into_iter().find(|v| v.id == id)
    }
}

/// What an admin sets on a field; `key` only when creating it.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldInput<'a> {
    pub key: Option<&'a str>,
    pub label_translations: &'a BTreeMap<String, String>,
    pub kind: Kind,
    pub options: &'a Options,
}

/// The fields of a field, all errors at once.
pub fn validate_field(input: &FieldInput<'_>) -> Result<(), Vec<FieldError>> {
    let mut v = Violations::new();
    if let Some(key) = input.key
        && !is_key(key)
    {
        v.add("key", "format");
    }
    add_errors(
        &mut v,
        i18n::validate_translations(
            "labelTranslations",
            input.label_translations,
            FIELD_LABEL_MAX_CHARS,
        ),
    );
    validate_options(&mut v, input.kind, input.options);
    v.into_result()
}

fn add_errors(v: &mut Violations, result: Result<(), Vec<FieldError>>) {
    if let Err(errors) = result {
        for error in errors {
            v.add(error.field, error.code);
        }
    }
}

fn validate_options(v: &mut Violations, kind: Kind, options: &Options) {
    for name in options.set() {
        if !kind.accepts(name) {
            v.add(format!("options.{}", name.as_str()), "not_applicable");
        }
    }
    if let Some(max_length) = options.max_length
        && !(1..=SHORT_TEXT_MAX_CHARS).contains(&max_length)
    {
        v.add("options.maxLength", "out_of_range");
    }
    if let Some(decimals) = options.decimals
        && decimals > MAX_DECIMALS
    {
        v.add("options.decimals", "out_of_range");
    }
    for (name, bound) in [("min", options.min), ("max", options.max)] {
        if bound.is_some_and(|b| !b.is_finite()) {
            v.add(format!("options.{name}"), "out_of_range");
        }
    }
    if let (Some(min), Some(max)) = (options.min, options.max)
        && min > max
    {
        v.add("options.max", "less_than_min");
    }
    if kind.has_values() {
        validate_values(v, kind, options.values.as_deref().unwrap_or_default());
    }
}

fn validate_values(v: &mut Violations, kind: Kind, values: &[ListValue]) {
    if values.is_empty() {
        v.add("options.values", "required");
        return;
    }
    let count: usize = values.iter().map(|value| 1 + value.children.len()).sum();
    if count > MAX_VALUES {
        v.add("options.values", "too_many");
    }
    let mut ids = BTreeSet::new();
    for (i, value) in values.iter().enumerate() {
        let path = format!("options.values[{i}]");
        validate_value_item(v, &path, value, &mut ids);
        if !value.children.is_empty() && kind != Kind::CascadingSelect {
            v.add(format!("{path}.children"), "not_applicable");
        }
        for (j, child) in value.children.iter().enumerate() {
            let child_path = format!("{path}.children[{j}]");
            validate_value_item(v, &child_path, child, &mut ids);
            if !child.children.is_empty() {
                v.add(format!("{child_path}.children"), "not_applicable");
            }
        }
    }
}

fn validate_value_item(v: &mut Violations, path: &str, value: &ListValue, ids: &mut BTreeSet<u128>) {
    if !ids.insert(value.id) {
        v.add(format!("{path}.id"), "duplicate");
    }
    add_errors(
        v,
        i18n::validate_translations(
            &format!("{path}.labelTranslations"),
            &value.labels,
            FIELD_LABEL_MAX_CHARS,
        ),
    );
}

/// The values `requested` for a field, with every value of its published
/// version that they leave out put back, deactivated: tickets may hold it.
/// A value added since (in the draft) goes for good. A published child left
/// out goes back under its parent, put back too if needed.
pub fn keep_published(requested: Vec<ListValue>, published: &[ListValue]) -> Vec<ListValue> {
    let mut kept = requested;
    for old in published {
        match kept.iter_mut().find(|v| v.id == old.id) {
            Some(value) => {
                for old_child in &old.children {
                    if !value.children.iter().any(|c| c.id == old_child.id) {
                        value.children.push(deactivated(old_child));
                    }
                }
            }
            None => kept.push(deactivated(old)),
        }
    }
    kept
}

fn deactivated(value: &ListValue) -> ListValue {
    ListValue {
        id: value.id,
        labels: value.labels.clone(),
        active: false,
        children: value.children.iter().map(deactivated).collect(),
    }
}

/// Whether a field is hidden, visible or required at a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Mode {
    Hidden,
    Visible,
    Required,
}

impl Mode {
    pub const ALL: [Self; 3] = [Self::Hidden, Self::Visible, Self::Required];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hidden => "hidden",
            Self::Visible => "visible",
            Self::Required => "required",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.as_str() == raw)
    }
}

/// A step of a ticket's life: its creation, or a status of its workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Step {
    Creation,
    Status(u128),
}

/// One cell of the rules grid of a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Rule {
    pub issue_type: u128,
    pub step: Step,
    pub mode: Mode,
}

/// Which types a field applies to, and its rules for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rules<'a> {
    /// Every type, including the ones added later.
    pub all_types: bool,
    /// The types, when not all of them.
    pub issue_types: &'a [u128],
    /// The cells that are not Visible.
    pub rules: &'a [Rule],
}

impl Rules<'_> {
    /// Whether the field applies to `issue_type`.
    pub fn applies_to(&self, issue_type: u128) -> bool {
        self.all_types || self.issue_types.contains(&issue_type)
    }
}

/// Whether the field is hidden, visible or required for a ticket of
/// `issue_type` at `step`: Hidden for a type it does not apply to, else the
/// rule of the cell, else Visible.
pub fn mode(rules: &Rules<'_>, issue_type: u128, step: Step) -> Mode {
    if !rules.applies_to(issue_type) {
        return Mode::Hidden;
    }
    rules
        .rules
        .iter()
        .find(|r| r.issue_type == issue_type && r.step == step)
        .map_or(Mode::Visible, |r| r.mode)
}

/// An issue type of the configuration, with the statuses of its workflow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeSteps {
    pub id: u128,
    pub statuses: Vec<u128>,
}

/// The rules an admin sets, checked against the types of the configuration:
/// what is stored (the cells that are not Visible, each once), or every
/// error, by its index in `requested`.
pub fn validate_rules(
    all_types: bool,
    issue_types: &[u128],
    requested: &[Rule],
    types: &[TypeSteps],
) -> Result<Vec<Rule>, Vec<FieldError>> {
    let mut v = Violations::new();
    for (i, id) in issue_types.iter().enumerate() {
        if !types.iter().any(|t| t.id == *id) {
            v.add(format!("issueTypeIds[{i}]"), "not_found");
        }
    }
    let rules = Rules {
        all_types,
        issue_types,
        rules: &[],
    };
    let mut seen = BTreeSet::new();
    let mut kept = Vec::new();
    for (i, rule) in requested.iter().enumerate() {
        let Some(steps) = types.iter().find(|t| t.id == rule.issue_type) else {
            v.add(format!("rules[{i}].issueTypeId"), "not_found");
            continue;
        };
        if !rules.applies_to(rule.issue_type) {
            v.add(format!("rules[{i}].issueTypeId"), "not_applicable");
            continue;
        }
        if let Step::Status(status) = rule.step
            && !steps.statuses.contains(&status)
        {
            v.add(format!("rules[{i}].statusId"), "not_found");
            continue;
        }
        if !seen.insert((rule.issue_type, rule.step)) {
            v.add(format!("rules[{i}]"), "duplicate");
            continue;
        }
        if rule.mode != Mode::Visible {
            kept.push(*rule);
        }
    }
    v.into_result().map(|()| {
        kept.sort();
        kept
    })
}

/// A value of a field on a ticket, as the API receives it (JSON).
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    Text(String),
    List(Vec<Value>),
}

impl Value {
    /// Whether it holds nothing: what a required field refuses. A checkbox
    /// left unticked holds `false`, which is a value.
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Null => true,
            Self::Text(text) => text.trim().is_empty(),
            Self::List(items) => items.is_empty(),
            Self::Bool(_) | Self::Number(_) => false,
        }
    }

    fn text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            _ => None,
        }
    }

    fn ids(&self) -> Vec<u128> {
        match self {
            Self::Text(text) => parse_uuid(text).into_iter().collect(),
            Self::List(items) => items.iter().flat_map(Self::ids).collect(),
            _ => Vec::new(),
        }
    }
}

/// Why a value does not fit its field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueError {
    /// Not the JSON type the field holds (text, number, list…).
    WrongType,
    TooLong,
    TooMany,
    OutOfRange,
    TooManyDecimals,
    InvalidDate,
    InvalidDateTime,
    InvalidUrl,
    InvalidLabel,
    /// Not an identifier (user, group, ticket, list value).
    InvalidId,
    /// No value of the list has this identifier.
    UnknownValue,
    /// A deactivated value, which only a ticket that had it may keep.
    InactiveValue,
    /// A second-level value that is not under the first one.
    NotAChild,
    Duplicate,
}

impl ValueError {
    /// A stable code, for the API and the translations.
    pub const fn code(self) -> &'static str {
        match self {
            Self::WrongType => "wrong_type",
            Self::TooLong => "too_long",
            Self::TooMany => "too_many",
            Self::OutOfRange => "out_of_range",
            Self::TooManyDecimals => "too_many_decimals",
            Self::InvalidDate => "invalid_date",
            Self::InvalidDateTime => "invalid_date_time",
            Self::InvalidUrl => "invalid_url",
            Self::InvalidLabel => "invalid_label",
            Self::InvalidId => "invalid_id",
            Self::UnknownValue => "unknown_value",
            Self::InactiveValue => "inactive_value",
            Self::NotAChild => "not_a_child",
            Self::Duplicate => "duplicate",
        }
    }
}

/// Whether `value` fits a field of `kind` with `options`. `previous` is
/// what the ticket held: a deactivated list value is accepted only if it
/// already had it. Empty values are always accepted here; whether the
/// field is required at a step is [`mode`]'s business. Users, groups and
/// tickets are checked to be identifiers; that they exist is the caller's.
pub fn validate_value(
    kind: Kind,
    options: &Options,
    value: &Value,
    previous: Option<&Value>,
) -> Result<(), ValueError> {
    if matches!(value, Value::Null) {
        return Ok(());
    }
    match kind {
        Kind::ShortText => {
            let text = value.text().ok_or(ValueError::WrongType)?;
            let max = options.max_length.unwrap_or(SHORT_TEXT_MAX_CHARS) as usize;
            if text.contains(['\n', '\r']) {
                Err(ValueError::WrongType)
            } else if text.chars().count() > max {
                Err(ValueError::TooLong)
            } else {
                Ok(())
            }
        }
        Kind::RichText => {
            let text = value.text().ok_or(ValueError::WrongType)?;
            if text.chars().count() > RICH_TEXT_MAX_CHARS {
                Err(ValueError::TooLong)
            } else {
                Ok(())
            }
        }
        Kind::Number => {
            let Value::Number(number) = value else {
                return Err(ValueError::WrongType);
            };
            validate_number(*number, options)
        }
        Kind::Checkbox => match value {
            Value::Bool(_) => Ok(()),
            _ => Err(ValueError::WrongType),
        },
        Kind::Date => {
            let text = value.text().ok_or(ValueError::WrongType)?;
            if is_date(text) {
                Ok(())
            } else {
                Err(ValueError::InvalidDate)
            }
        }
        Kind::DateTime => {
            let text = value.text().ok_or(ValueError::WrongType)?;
            if is_date_time(text) {
                Ok(())
            } else {
                Err(ValueError::InvalidDateTime)
            }
        }
        Kind::SingleSelect => {
            let text = value.text().ok_or(ValueError::WrongType)?;
            pick(options, text, previous, None).map(|_| ())
        }
        Kind::MultiSelect => {
            let items = list(value)?;
            let mut seen = BTreeSet::new();
            for item in items {
                let text = item.text().ok_or(ValueError::WrongType)?;
                if !seen.insert(pick(options, text, previous, None)?) {
                    return Err(ValueError::Duplicate);
                }
            }
            Ok(())
        }
        Kind::CascadingSelect => validate_cascade(options, value, previous),
        Kind::User if options.multiple == Some(true) => {
            let items = list(value)?;
            let mut seen = BTreeSet::new();
            for item in items {
                let id = item.text().and_then(parse_uuid).ok_or(ValueError::InvalidId)?;
                if !seen.insert(id) {
                    return Err(ValueError::Duplicate);
                }
            }
            Ok(())
        }
        Kind::User | Kind::Group | Kind::IssueLink => {
            let text = value.text().ok_or(ValueError::WrongType)?;
            parse_uuid(text).map(|_| ()).ok_or(ValueError::InvalidId)
        }
        Kind::Labels => validate_labels(value),
        Kind::Url => {
            let text = value.text().ok_or(ValueError::WrongType)?;
            if text.chars().count() > URL_MAX_CHARS {
                Err(ValueError::TooLong)
            } else if is_url(text) {
                Ok(())
            } else {
                Err(ValueError::InvalidUrl)
            }
        }
    }
}

fn list(value: &Value) -> Result<&[Value], ValueError> {
    match value {
        Value::List(items) => Ok(items),
        _ => Err(ValueError::WrongType),
    }
}

fn validate_number(number: f64, options: &Options) -> Result<(), ValueError> {
    if !number.is_finite()
        || options.min.is_some_and(|min| number < min)
        || options.max.is_some_and(|max| number > max)
    {
        return Err(ValueError::OutOfRange);
    }
    let decimals = options.decimals.unwrap_or(0).min(MAX_DECIMALS);
    let scaled = number * 10f64.powi(i32::from(decimals));
    if (scaled - scaled.round()).abs() > 1e-6 * scaled.abs().max(1.0) {
        Err(ValueError::TooManyDecimals)
    } else {
        Ok(())
    }
}

/// The list value `text` names, among `among` (the first level if none):
/// active, or held before.
fn pick(
    options: &Options,
    text: &str,
    previous: Option<&Value>,
    among: Option<&ListValue>,
) -> Result<u128, ValueError> {
    let id = parse_uuid(text).ok_or(ValueError::InvalidId)?;
    let found = match among {
        Some(parent) => parent.children.iter().find(|v| v.id == id),
        None => options.values.iter().flatten().find(|v| v.id == id),
    };
    let Some(value) = found else {
        return Err(if among.is_some() && options.value(id).is_some() {
            ValueError::NotAChild
        } else {
            ValueError::UnknownValue
        });
    };
    if !value.active && !previous.is_some_and(|p| p.ids().contains(&id)) {
        return Err(ValueError::InactiveValue);
    }
    Ok(id)
}

/// A cascading select holds its first-level value, then optionally one of
/// the values under it: `[parent]` or `[parent, child]`.
fn validate_cascade(options: &Options, value: &Value, previous: Option<&Value>) -> Result<(), ValueError> {
    let items = list(value)?;
    if items.len() > 2 {
        return Err(ValueError::TooMany);
    }
    let Some(first) = items.first() else {
        return Ok(());
    };
    let parent_id = pick(
        options,
        first.text().ok_or(ValueError::WrongType)?,
        previous,
        None,
    )?;
    if let Some(second) = items.get(1) {
        let parent = options.value(parent_id).ok_or(ValueError::UnknownValue)?;
        pick(
            options,
            second.text().ok_or(ValueError::WrongType)?,
            previous,
            Some(parent),
        )?;
    }
    Ok(())
}

fn validate_labels(value: &Value) -> Result<(), ValueError> {
    let items = list(value)?;
    if items.len() > MAX_LABELS {
        return Err(ValueError::TooMany);
    }
    let mut seen = BTreeSet::new();
    for item in items {
        let label = item.text().ok_or(ValueError::WrongType)?;
        if label.is_empty()
            || label.chars().count() > LABEL_MAX_CHARS
            || label.chars().any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(ValueError::InvalidLabel);
        }
        if !seen.insert(label) {
            return Err(ValueError::Duplicate);
        }
    }
    Ok(())
}

/// Whether `text` is an absolute http(s) URL with a host and no spaces.
fn is_url(text: &str) -> bool {
    let rest = text
        .strip_prefix("https://")
        .or_else(|| text.strip_prefix("http://"));
    let Some(rest) = rest else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    !host.is_empty()
        && !host.starts_with(['.', ':'])
        && !text.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// Parses `n` ASCII digits.
fn digits(text: &str) -> Option<u32> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        2 if (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Whether `text` is a calendar date, `YYYY-MM-DD`.
pub fn is_date(text: &str) -> bool {
    let parts: Vec<&str> = text.split('-').collect();
    let [year, month, day] = parts.as_slice() else {
        return false;
    };
    if year.len() != 4 || month.len() != 2 || day.len() != 2 {
        return false;
    }
    match (digits(year), digits(month), digits(day)) {
        (Some(y), Some(m), Some(d)) => y >= 1 && (1..=12).contains(&m) && d >= 1 && d <= days_in_month(y, m),
        _ => false,
    }
}

/// Whether `text` is an RFC 3339 date-time with its offset:
/// `YYYY-MM-DDTHH:MM:SS[.fraction](Z|±HH:MM)`.
pub fn is_date_time(text: &str) -> bool {
    let Some((date, time)) = text.split_once(['T', 't']) else {
        return false;
    };
    if !is_date(date) {
        return false;
    }
    let (clock, offset) = if let Some(clock) = time.strip_suffix(['Z', 'z']) {
        (clock, None)
    } else {
        let Some(at) = time.rfind(['+', '-']) else {
            return false;
        };
        (&time[..at], Some(&time[at + 1..]))
    };
    let (clock, fraction) = clock.split_once('.').unwrap_or((clock, "0"));
    let parts: Vec<&str> = clock.split(':').collect();
    let [hours, minutes, seconds] = parts.as_slice() else {
        return false;
    };
    let two = |s: &str, max: u32| s.len() == 2 && digits(s).is_some_and(|n| n <= max);
    let offset_ok = offset.is_none_or(|o| match o.split_once(':') {
        Some((h, m)) => two(h, 23) && two(m, 59),
        None => false,
    });
    two(hours, 23) && two(minutes, 59) && two(seconds, 60) && digits(fraction).is_some() && offset_ok
}

/// The identifier `text` spells, in its hyphenated form
/// (`01933b2e-7c4a-7b9e-8f3d-2a6c5e1f0b9a`).
pub fn parse_uuid(text: &str) -> Option<u128> {
    let bytes = text.as_bytes();
    if bytes.len() != 36 {
        return None;
    }
    let mut id: u128 = 0;
    for (i, byte) in bytes.iter().enumerate() {
        if matches!(i, 8 | 13 | 18 | 23) {
            if *byte != b'-' {
                return None;
            }
            continue;
        }
        let digit = char::from(*byte).to_digit(16)?;
        id = (id << 4) | u128::from(digit);
    }
    Some(id)
}

/// A system field: every configuration has it, it cannot be removed, and
/// only the types it applies to and its rules change.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SystemField {
    pub key: &'static str,
    pub label_fr: &'static str,
    pub label_en: &'static str,
    pub kind: Kind,
    pub decimals: u8,
    pub unit: Option<Unit>,
    /// Applies to every type, including the ones added later.
    pub all_types: bool,
    /// Keys of the default types it applies to, when not all.
    pub type_keys: &'static [&'static str],
}

/// The system fields (`app_seed_default_fields`; an integration test keeps
/// both in step), in their order.
pub const SYSTEM_FIELDS: [SystemField; 2] = [
    SystemField {
        key: "story_points",
        label_fr: "Points",
        label_en: "Points",
        kind: Kind::Number,
        decimals: 1,
        unit: None,
        all_types: false,
        type_keys: &["story", "task"],
    },
    SystemField {
        key: "remaining",
        label_fr: "Reste à faire",
        label_en: "Remaining work",
        kind: Kind::Number,
        decimals: 2,
        unit: Some(Unit::Hours),
        all_types: true,
        type_keys: &[],
    },
];

/// Whether `key` names a system field.
pub fn is_system_key(key: &str) -> bool {
    SYSTEM_FIELDS.iter().any(|f| f.key == key)
}

#[cfg(test)]
mod tests {
    use super::*;

    const STORY: u128 = 1;
    const BUG: u128 = 2;
    const TASK: u128 = 3;
    const IN_PROGRESS: u128 = 20;
    const IN_REVIEW: u128 = 21;
    const DONE: u128 = 22;
    const OTHER_STATUS: u128 = 30;

    const DIRECTORY: u128 = 0x0193_3b2e_7c4a_7b9e_8f3d_2a6c_5e1f_0b01;
    const MAIL: u128 = 0x0193_3b2e_7c4a_7b9e_8f3d_2a6c_5e1f_0b02;
    const ERP: u128 = 0x0193_3b2e_7c4a_7b9e_8f3d_2a6c_5e1f_0b03;
    const ERP_FINANCE: u128 = 0x0193_3b2e_7c4a_7b9e_8f3d_2a6c_5e1f_0b04;

    fn uuid(id: u128) -> String {
        let hex = format!("{id:032x}");
        format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        )
    }

    fn labels(fr: &str, en: &str) -> BTreeMap<String, String> {
        [("fr".to_owned(), fr.to_owned()), ("en".to_owned(), en.to_owned())].into()
    }

    fn value(id: u128, fr: &str, en: &str) -> ListValue {
        ListValue {
            id,
            labels: labels(fr, en),
            active: true,
            children: Vec::new(),
        }
    }

    /// Application: Directory, Mail, ERP (Finance under ERP for a cascade).
    fn application(cascade: bool) -> Options {
        let mut erp = value(ERP, "ERP", "ERP");
        if cascade {
            erp.children.push(value(ERP_FINANCE, "Finance", "Finance"));
        }
        Options {
            values: Some(vec![
                value(DIRECTORY, "Annuaire", "Directory"),
                value(MAIL, "Messagerie", "Mail"),
                erp,
            ]),
            ..Options::default()
        }
    }

    fn text(s: &str) -> Value {
        Value::Text(s.to_owned())
    }

    fn check(kind: Kind, options: &Options, value: &Value) -> Result<(), ValueError> {
        validate_value(kind, options, value, None)
    }

    fn input<'a>(
        key: Option<&'a str>,
        labels: &'a BTreeMap<String, String>,
        kind: Kind,
        options: &'a Options,
    ) -> FieldInput<'a> {
        FieldInput {
            key,
            label_translations: labels,
            kind,
            options,
        }
    }

    fn errors(result: Result<(), Vec<FieldError>>) -> Vec<(String, &'static str)> {
        result
            .err()
            .unwrap_or_default()
            .into_iter()
            .map(|e| (e.field, e.code))
            .collect()
    }

    #[test]
    fn kinds_units_and_modes_round_trip() {
        for kind in Kind::ALL {
            assert_eq!(Kind::parse(kind.as_str()), Some(kind));
        }
        for unit in Unit::ALL {
            assert_eq!(Unit::parse(unit.as_str()), Some(unit));
        }
        for mode in Mode::ALL {
            assert_eq!(Mode::parse(mode.as_str()), Some(mode));
        }
        assert_eq!(Kind::parse("formula"), None);
        assert_eq!(Mode::parse("optional"), None);
        let lists: Vec<Kind> = Kind::ALL.into_iter().filter(|k| k.has_values()).collect();
        assert_eq!(
            lists,
            [Kind::SingleSelect, Kind::MultiSelect, Kind::CascadingSelect]
        );
    }

    #[test]
    fn a_single_select_with_its_values_is_valid() {
        let names = labels("Application", "Application");
        let options = application(false);
        assert_eq!(
            validate_field(&input(Some("application"), &names, Kind::SingleSelect, &options)),
            Ok(())
        );
    }

    #[test]
    fn a_field_reports_every_invalid_field() {
        let names: BTreeMap<String, String> = [("de".to_owned(), "Anwendung".to_owned())].into();
        let options = Options::default();
        assert_eq!(
            errors(validate_field(&input(
                Some("Application"),
                &names,
                Kind::SingleSelect,
                &options
            ))),
            [
                ("key".to_owned(), "format"),
                ("labelTranslations".to_owned(), "required"),
                ("labelTranslations.de".to_owned(), "unsupported_locale"),
                ("options.values".to_owned(), "required"),
            ]
        );
        let names = labels("Écran", "Screen");
        assert_eq!(
            validate_field(&input(None, &names, Kind::Checkbox, &options)),
            Ok(()),
            "no key when changing a field"
        );
    }

    #[test]
    fn each_type_takes_its_own_options_only() {
        let names = labels("Champ", "Field");
        let every = Options {
            max_length: Some(10),
            decimals: Some(1),
            min: Some(0.0),
            max: Some(10.0),
            unit: Some(Unit::Hours),
            multiple: Some(true),
            values: Some(vec![value(DIRECTORY, "Annuaire", "Directory")]),
        };
        let accepted = |kind: Kind| -> Vec<&'static str> {
            let refused: Vec<String> = errors(validate_field(&input(None, &names, kind, &every)))
                .into_iter()
                .filter(|(_, code)| *code == "not_applicable")
                .map(|(field, _)| field)
                .collect();
            [
                "maxLength",
                "decimals",
                "min",
                "max",
                "unit",
                "multiple",
                "values",
            ]
            .into_iter()
            .filter(|name| !refused.contains(&format!("options.{name}")))
            .collect()
        };
        assert_eq!(accepted(Kind::ShortText), ["maxLength"]);
        assert_eq!(accepted(Kind::Number), ["decimals", "min", "max", "unit"]);
        assert_eq!(accepted(Kind::User), ["multiple"]);
        for kind in [Kind::SingleSelect, Kind::MultiSelect, Kind::CascadingSelect] {
            assert_eq!(accepted(kind), ["values"], "{kind:?}");
        }
        for kind in [
            Kind::RichText,
            Kind::Checkbox,
            Kind::Date,
            Kind::DateTime,
            Kind::Group,
            Kind::Labels,
            Kind::Url,
            Kind::IssueLink,
        ] {
            assert!(accepted(kind).is_empty(), "{kind:?}");
            assert_eq!(
                validate_field(&input(None, &names, kind, &Options::default())),
                Ok(()),
                "{kind:?} needs no option"
            );
        }
    }

    #[test]
    fn options_stay_within_their_bounds() {
        let names = labels("Champ", "Field");
        let short = Options {
            max_length: Some(0),
            ..Options::default()
        };
        assert_eq!(
            errors(validate_field(&input(None, &names, Kind::ShortText, &short))),
            [("options.maxLength".to_owned(), "out_of_range")]
        );
        let number = Options {
            decimals: Some(5),
            min: Some(10.0),
            max: Some(f64::INFINITY),
            ..Options::default()
        };
        assert_eq!(
            errors(validate_field(&input(None, &names, Kind::Number, &number))),
            [
                ("options.decimals".to_owned(), "out_of_range"),
                ("options.max".to_owned(), "out_of_range"),
            ]
        );
        let inverted = Options {
            min: Some(10.0),
            max: Some(1.0),
            ..Options::default()
        };
        assert_eq!(
            errors(validate_field(&input(None, &names, Kind::Number, &inverted))),
            [("options.max".to_owned(), "less_than_min")]
        );
    }

    #[test]
    fn list_values_are_named_unique_and_nested_only_in_a_cascade() {
        let names = labels("Application", "Application");
        let mut options = application(true);
        assert_eq!(
            errors(validate_field(&input(None, &names, Kind::SingleSelect, &options))),
            [("options.values[2].children".to_owned(), "not_applicable")]
        );
        assert_eq!(
            validate_field(&input(None, &names, Kind::CascadingSelect, &options)),
            Ok(())
        );
        let values = options.values.as_mut().unwrap();
        values[1].id = DIRECTORY;
        values[2].children[0].labels = BTreeMap::new();
        values[2].children[0].children.push(value(MAIL, "x", "x"));
        assert_eq!(
            errors(validate_field(&input(
                None,
                &names,
                Kind::CascadingSelect,
                &options
            ))),
            [
                ("options.values[1].id".to_owned(), "duplicate"),
                (
                    "options.values[2].children[0].labelTranslations".to_owned(),
                    "required"
                ),
                (
                    "options.values[2].children[0].children".to_owned(),
                    "not_applicable"
                ),
            ]
        );
        let many = Options {
            values: Some(
                (0..=u128::try_from(MAX_VALUES).unwrap())
                    .map(|id| value(id, "v", "v"))
                    .collect(),
            ),
            ..Options::default()
        };
        assert_eq!(
            errors(validate_field(&input(None, &names, Kind::MultiSelect, &many))),
            [("options.values".to_owned(), "too_many")]
        );
    }

    #[test]
    fn published_values_left_out_are_deactivated_and_new_ones_go() {
        let published = application(true).values.unwrap();
        // ERP removed, Mail renamed, a new value added then left out again.
        let mut requested = vec![
            value(DIRECTORY, "Annuaire", "Directory"),
            value(MAIL, "Courriel", "Email"),
        ];
        let kept = keep_published(requested.clone(), &published);
        assert_eq!(kept.len(), 3);
        assert_eq!(kept[1].labels, labels("Courriel", "Email"), "the change stays");
        assert_eq!(kept[2].id, ERP);
        assert!(!kept[2].active && !kept[2].children[0].active);
        // A value of the draft only goes for good.
        requested.push(value(0xdead, "Nouveau", "New"));
        let kept = keep_published(requested[..2].to_vec(), &published);
        assert!(kept.iter().all(|v| v.id != 0xdead));
        // A published child left out goes back under its parent.
        let erp_without_child = value(ERP, "ERP", "ERP");
        let kept = keep_published(vec![erp_without_child], &published);
        let erp = kept.iter().find(|v| v.id == ERP).unwrap();
        assert!(erp.active);
        assert_eq!(erp.children.len(), 1);
        assert!(!erp.children[0].active);
        // Deactivated, still readable.
        let options = Options {
            values: Some(kept),
            ..Options::default()
        };
        assert!(options.value(ERP_FINANCE).is_some_and(|v| !v.active));
        assert!(options.value(DIRECTORY).is_some_and(|v| !v.active));
    }

    fn grid() -> Vec<TypeSteps> {
        let standard = vec![IN_PROGRESS, IN_REVIEW, DONE];
        vec![
            TypeSteps {
                id: STORY,
                statuses: standard.clone(),
            },
            TypeSteps {
                id: BUG,
                statuses: standard.clone(),
            },
            TypeSteps {
                id: TASK,
                statuses: standard,
            },
        ]
    }

    fn rule(issue_type: u128, step: Step, mode: Mode) -> Rule {
        Rule {
            issue_type,
            step,
            mode,
        }
    }

    #[test]
    fn rules_resolve_per_type_and_step_with_visible_by_default() {
        // Mockup 12: Bug required at creation, Task hidden there.
        let stored = [
            rule(BUG, Step::Creation, Mode::Required),
            rule(TASK, Step::Creation, Mode::Hidden),
            rule(STORY, Step::Status(DONE), Mode::Required),
        ];
        let rules = Rules {
            all_types: false,
            issue_types: &[STORY, BUG, TASK],
            rules: &stored,
        };
        assert_eq!(mode(&rules, BUG, Step::Creation), Mode::Required);
        assert_eq!(mode(&rules, TASK, Step::Creation), Mode::Hidden);
        assert_eq!(mode(&rules, TASK, Step::Status(IN_PROGRESS)), Mode::Visible);
        assert_eq!(mode(&rules, STORY, Step::Status(DONE)), Mode::Required);
        // A status added later: visible.
        assert_eq!(mode(&rules, STORY, Step::Status(OTHER_STATUS)), Mode::Visible);
        // A type it does not apply to: hidden everywhere.
        let only_bug = Rules {
            issue_types: &[BUG],
            ..rules
        };
        assert_eq!(mode(&only_bug, STORY, Step::Status(DONE)), Mode::Hidden);
        // All types, including one added later.
        let all = Rules {
            all_types: true,
            issue_types: &[],
            rules: &[],
        };
        assert_eq!(mode(&all, 99, Step::Creation), Mode::Visible);
    }

    #[test]
    fn rules_keep_the_cells_that_are_not_visible() {
        let requested = [
            rule(TASK, Step::Creation, Mode::Hidden),
            rule(BUG, Step::Creation, Mode::Required),
            rule(BUG, Step::Status(IN_PROGRESS), Mode::Visible),
        ];
        assert_eq!(
            validate_rules(false, &[STORY, BUG, TASK], &requested, &grid()),
            Ok(vec![
                rule(BUG, Step::Creation, Mode::Required),
                rule(TASK, Step::Creation, Mode::Hidden),
            ])
        );
    }

    #[test]
    fn rules_name_types_and_statuses_of_their_workflow_once() {
        let requested = [
            rule(99, Step::Creation, Mode::Hidden),
            rule(STORY, Step::Creation, Mode::Hidden),
            rule(BUG, Step::Status(OTHER_STATUS), Mode::Required),
            rule(BUG, Step::Creation, Mode::Required),
            rule(BUG, Step::Creation, Mode::Hidden),
        ];
        let refused = validate_rules(false, &[BUG, 98], &requested, &grid()).unwrap_err();
        let fields: Vec<(&str, &str)> = refused.iter().map(|e| (e.field.as_str(), e.code)).collect();
        assert_eq!(
            fields,
            [
                ("issueTypeIds[1]", "not_found"),
                ("rules[0].issueTypeId", "not_found"),
                ("rules[1].issueTypeId", "not_applicable"),
                ("rules[2].statusId", "not_found"),
                ("rules[4]", "duplicate"),
            ]
        );
        // Every type, when the field applies to all of them.
        assert!(validate_rules(true, &[], &requested[1..2], &grid()).is_ok());
    }

    #[test]
    fn short_and_rich_texts_have_their_limits() {
        let ten = Options {
            max_length: Some(10),
            ..Options::default()
        };
        assert_eq!(check(Kind::ShortText, &ten, &text("Annuaire")), Ok(()));
        assert_eq!(
            check(Kind::ShortText, &ten, &text("é".repeat(11).as_str())),
            Err(ValueError::TooLong)
        );
        assert_eq!(
            check(Kind::ShortText, &ten, &text("two\nlines")),
            Err(ValueError::WrongType)
        );
        assert_eq!(
            check(Kind::ShortText, &Options::default(), &text(&"a".repeat(255))),
            Ok(())
        );
        assert_eq!(
            check(Kind::ShortText, &ten, &Value::Number(1.0)),
            Err(ValueError::WrongType)
        );
        let none = Options::default();
        assert_eq!(check(Kind::RichText, &none, &text("**bold**\nline")), Ok(()));
        assert_eq!(
            check(Kind::RichText, &none, &text(&"a".repeat(RICH_TEXT_MAX_CHARS + 1))),
            Err(ValueError::TooLong)
        );
    }

    #[test]
    fn numbers_respect_bounds_and_decimals() {
        let points = Options {
            decimals: Some(1),
            min: Some(0.0),
            max: Some(100.0),
            ..Options::default()
        };
        for ok in [0.0, 0.5, 13.0, 100.0, 2.3] {
            assert_eq!(check(Kind::Number, &points, &Value::Number(ok)), Ok(()), "{ok}");
        }
        assert_eq!(
            check(Kind::Number, &points, &Value::Number(0.25)),
            Err(ValueError::TooManyDecimals)
        );
        assert_eq!(
            check(Kind::Number, &points, &Value::Number(-1.0)),
            Err(ValueError::OutOfRange)
        );
        assert_eq!(
            check(Kind::Number, &points, &Value::Number(100.5)),
            Err(ValueError::OutOfRange)
        );
        assert_eq!(
            check(Kind::Number, &points, &Value::Number(f64::NAN)),
            Err(ValueError::OutOfRange)
        );
        assert_eq!(
            check(Kind::Number, &points, &text("3")),
            Err(ValueError::WrongType)
        );
        let integers = Options::default();
        assert_eq!(
            check(Kind::Number, &integers, &Value::Number(1.5)),
            Err(ValueError::TooManyDecimals)
        );
        assert_eq!(check(Kind::Number, &integers, &Value::Number(-42.0)), Ok(()));
    }

    #[test]
    fn checkboxes_dates_and_date_times_have_their_formats() {
        let none = Options::default();
        assert_eq!(check(Kind::Checkbox, &none, &Value::Bool(false)), Ok(()));
        assert_eq!(
            check(Kind::Checkbox, &none, &text("true")),
            Err(ValueError::WrongType)
        );
        for ok in ["2026-11-14", "2024-02-29", "2000-02-29", "0001-01-01"] {
            assert_eq!(check(Kind::Date, &none, &text(ok)), Ok(()), "{ok}");
        }
        for bad in [
            "2026-02-29",
            "1900-02-29",
            "2026-13-01",
            "2026-04-31",
            "2026-1-01",
            "26-11-14",
            "2026-11-14T10:00:00Z",
        ] {
            assert_eq!(
                check(Kind::Date, &none, &text(bad)),
                Err(ValueError::InvalidDate),
                "{bad}"
            );
        }
        for ok in [
            "2026-11-14T09:30:00Z",
            "2026-11-14T09:30:00.123+01:00",
            "2026-03-29T02:30:00-05:00",
            "2026-12-31T23:59:60Z",
        ] {
            assert_eq!(check(Kind::DateTime, &none, &text(ok)), Ok(()), "{ok}");
        }
        for bad in [
            "2026-11-14",
            "2026-11-14T09:30Z",
            "2026-11-14T09:30:00",
            "2026-11-14T24:00:00Z",
            "2026-11-14T09:30:00+1:00",
            "2026-02-30T09:30:00Z",
        ] {
            assert_eq!(
                check(Kind::DateTime, &none, &text(bad)),
                Err(ValueError::InvalidDateTime),
                "{bad}"
            );
        }
    }

    #[test]
    fn single_and_multi_selects_pick_active_values_of_their_list() {
        let mut options = application(false);
        assert_eq!(check(Kind::SingleSelect, &options, &text(&uuid(MAIL))), Ok(()));
        assert_eq!(
            check(Kind::SingleSelect, &options, &text(&uuid(0xbeef))),
            Err(ValueError::UnknownValue)
        );
        assert_eq!(
            check(Kind::SingleSelect, &options, &text("Mail")),
            Err(ValueError::InvalidId)
        );
        let both = Value::List(vec![text(&uuid(MAIL)), text(&uuid(ERP))]);
        assert_eq!(check(Kind::MultiSelect, &options, &both), Ok(()));
        let twice = Value::List(vec![text(&uuid(MAIL)), text(&uuid(MAIL))]);
        assert_eq!(
            check(Kind::MultiSelect, &options, &twice),
            Err(ValueError::Duplicate)
        );
        assert_eq!(
            check(Kind::MultiSelect, &options, &text(&uuid(MAIL))),
            Err(ValueError::WrongType)
        );
        // Mail deactivated: refused for a new value, kept by a ticket that had it.
        options.values.as_mut().unwrap()[1].active = false;
        let mail = text(&uuid(MAIL));
        assert_eq!(
            check(Kind::SingleSelect, &options, &mail),
            Err(ValueError::InactiveValue)
        );
        assert_eq!(
            validate_value(Kind::SingleSelect, &options, &mail, Some(&mail)),
            Ok(())
        );
        assert_eq!(
            validate_value(
                Kind::MultiSelect,
                &options,
                &both,
                Some(&Value::List(vec![mail.clone()]))
            ),
            Ok(())
        );
    }

    #[test]
    fn a_cascading_select_holds_a_value_then_one_under_it() {
        let options = application(true);
        let erp = text(&uuid(ERP));
        let finance = text(&uuid(ERP_FINANCE));
        assert_eq!(
            check(Kind::CascadingSelect, &options, &Value::List(vec![erp.clone()])),
            Ok(())
        );
        assert_eq!(
            check(
                Kind::CascadingSelect,
                &options,
                &Value::List(vec![erp.clone(), finance.clone()])
            ),
            Ok(())
        );
        assert_eq!(
            check(
                Kind::CascadingSelect,
                &options,
                &Value::List(vec![text(&uuid(MAIL)), finance.clone()])
            ),
            Err(ValueError::NotAChild)
        );
        assert_eq!(
            check(
                Kind::CascadingSelect,
                &options,
                &Value::List(vec![finance.clone()])
            ),
            Err(ValueError::UnknownValue),
            "a child is no first-level value"
        );
        assert_eq!(
            check(
                Kind::CascadingSelect,
                &options,
                &Value::List(vec![erp.clone(), finance.clone(), finance])
            ),
            Err(ValueError::TooMany)
        );
        assert_eq!(
            check(Kind::CascadingSelect, &options, &erp),
            Err(ValueError::WrongType)
        );
    }

    #[test]
    fn users_groups_and_ticket_links_are_identifiers() {
        let id = text("01933b2e-7c4a-7b9e-8f3d-2a6c5e1f0b9a");
        let one = Options::default();
        for kind in [Kind::User, Kind::Group, Kind::IssueLink] {
            assert_eq!(check(kind, &one, &id), Ok(()), "{kind:?}");
            assert_eq!(
                check(kind, &one, &text("PROJ-12")),
                Err(ValueError::InvalidId),
                "{kind:?}"
            );
        }
        let several = Options {
            multiple: Some(true),
            ..Options::default()
        };
        assert_eq!(
            check(Kind::User, &several, &Value::List(vec![id.clone()])),
            Ok(())
        );
        assert_eq!(
            check(Kind::User, &several, &Value::List(vec![id.clone(), id.clone()])),
            Err(ValueError::Duplicate)
        );
        assert_eq!(check(Kind::User, &several, &id), Err(ValueError::WrongType));
        assert_eq!(
            check(Kind::User, &one, &Value::List(vec![id])),
            Err(ValueError::WrongType)
        );
    }

    #[test]
    fn labels_are_distinct_words_and_urls_are_web_addresses() {
        let none = Options::default();
        let tags = |items: &[&str]| Value::List(items.iter().map(|s| text(s)).collect());
        assert_eq!(check(Kind::Labels, &none, &tags(&["frontend", "sso"])), Ok(()));
        assert_eq!(
            check(Kind::Labels, &none, &tags(&["two words"])),
            Err(ValueError::InvalidLabel)
        );
        assert_eq!(
            check(Kind::Labels, &none, &tags(&[""])),
            Err(ValueError::InvalidLabel)
        );
        assert_eq!(
            check(Kind::Labels, &none, &tags(&["sso", "sso"])),
            Err(ValueError::Duplicate)
        );
        let many: Vec<String> = (0..=MAX_LABELS).map(|i| format!("l{i}")).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        assert_eq!(check(Kind::Labels, &none, &tags(&many)), Err(ValueError::TooMany));
        for ok in ["https://example.org", "http://intranet:8080/a?b=c#d"] {
            assert_eq!(check(Kind::Url, &none, &text(ok)), Ok(()), "{ok}");
        }
        for bad in [
            "example.org",
            "ftp://example.org",
            "https://",
            "https://exa mple.org",
            "javascript:alert(1)",
        ] {
            assert_eq!(
                check(Kind::Url, &none, &text(bad)),
                Err(ValueError::InvalidUrl),
                "{bad}"
            );
        }
        let long = format!("https://example.org/{}", "a".repeat(URL_MAX_CHARS));
        assert_eq!(check(Kind::Url, &none, &text(&long)), Err(ValueError::TooLong));
    }

    #[test]
    fn empty_values_fit_any_field_and_are_what_required_refuses() {
        for kind in Kind::ALL {
            assert_eq!(check(kind, &application(true), &Value::Null), Ok(()), "{kind:?}");
        }
        assert!(Value::Null.is_empty());
        assert!(text("  ").is_empty());
        assert!(Value::List(Vec::new()).is_empty());
        assert!(!Value::Bool(false).is_empty());
        assert!(!Value::Number(0.0).is_empty());
    }

    #[test]
    fn identifiers_parse_in_their_hyphenated_form() {
        assert_eq!(parse_uuid(&uuid(DIRECTORY)), Some(DIRECTORY));
        assert_eq!(
            parse_uuid("01933B2E-7C4A-7B9E-8F3D-2A6C5E1F0B01"),
            Some(DIRECTORY)
        );
        assert_eq!(parse_uuid("01933b2e7c4a7b9e8f3d2a6c5e1f0b01"), None);
        assert_eq!(parse_uuid("01933b2e-7c4a-7b9e-8f3d-2a6c5e1f0bzz"), None);
    }

    #[test]
    fn system_fields_are_valid_numbers_on_default_types() {
        let defaults: Vec<&str> = crate::issue_types::DEFAULT_TYPES.iter().map(|t| t.key).collect();
        for field in SYSTEM_FIELDS {
            assert!(is_key(field.key) && is_system_key(field.key), "{}", field.key);
            let options = Options {
                decimals: Some(field.decimals),
                unit: field.unit,
                ..Options::default()
            };
            let names = labels(field.label_fr, field.label_en);
            assert_eq!(
                validate_field(&input(Some(field.key), &names, field.kind, &options)),
                Ok(())
            );
            assert!(field.all_types != !field.type_keys.is_empty());
            assert!(
                field.type_keys.iter().all(|k| defaults.contains(k)),
                "{}",
                field.key
            );
        }
        assert!(!is_system_key("application"));
    }
}
