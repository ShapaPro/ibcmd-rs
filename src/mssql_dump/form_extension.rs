//! A form an extension adopts: its event interceptors (`callType`) and the
//! base form it adopted (`<BaseForm>`).
//!
//! An event block stores every handler of every event:
//! `{N,(event,"first handler")xN,1,0,(event,code,n,("handler",code)x(n-1))xN}`;
//! code `0` is `Before`, `1` `After`, `2` `Override`. A configuration's form
//! stores the same block with one handler per event and writes no call type;
//! an adopted form (and the base form inside it) writes one on every event
//! (fixture `adopted/form_events`: `Before`/`After`/`Override` interceptors,
//! the base form's own events `Before`; ИТК: an interceptor with an empty
//! first handler and one `After` handler).
//!
//! The adopted form's body ends `…,{0,0}x4,1,<base form body>,0` where a
//! form body ends `…,{0,0}x4,0,0`; the base form body is a whole form body.

use std::collections::BTreeMap;

use anyhow::{Context, Result};

use super::*;

/// One handler of one event, as stored.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct FormEventBinding {
    pub(super) event: String,
    pub(super) handler: String,
    pub(super) code: u8,
}

const CALL_TYPES: [&str; 3] = ["Before", "After", "Override"];

/// The bindings of the event block `field`, in stored order: an event's first
/// handler (when it has one), then its further handlers.
pub(super) fn parse_form_event_block(field: &str) -> Option<Vec<FormEventBinding>> {
    let fields = split_1c_braced_fields(field.trim(), 0)?;
    let count: usize = fields.first()?.trim().parse().ok()?;
    if count == 0 || count > fields.len() || fields.len() < 3 + 5 * count {
        return None;
    }
    let mut first = Vec::with_capacity(count);
    for index in 0..count {
        let event = fields.get(1 + 2 * index)?.trim();
        let (handler, _) = parse_1c_quoted_string_with_len(fields.get(2 + 2 * index)?.trim())?;
        first.push((event, handler));
    }
    let mut at = 1 + 2 * count;
    if fields.get(at)?.trim() != "1" || fields.get(at + 1)?.trim() != "0" {
        return None;
    }
    at += 2;
    let mut out = Vec::new();
    for (event, handler) in first {
        if fields.get(at)?.trim() != event {
            return None;
        }
        let code: u8 = fields.get(at + 1)?.trim().parse().ok()?;
        let handlers: usize = fields.get(at + 2)?.trim().parse().ok()?;
        if handlers == 0 || usize::from(code) >= CALL_TYPES.len() {
            return None;
        }
        at += 3;
        if !handler.is_empty() {
            out.push(FormEventBinding {
                event: event.to_owned(),
                handler,
                code,
            });
        }
        for _ in 1..handlers {
            let (extra, _) = parse_1c_quoted_string_with_len(fields.get(at)?.trim())?;
            let code: u8 = fields.get(at + 1)?.trim().parse().ok()?;
            if usize::from(code) >= CALL_TYPES.len() {
                return None;
            }
            at += 2;
            out.push(FormEventBinding {
                event: event.to_owned(),
                handler: extra,
                code,
            });
        }
    }
    (at == fields.len()).then_some(out)
}

/// Call types of the handlers of a form body: by event and handler, and by
/// handler alone for an event whose name the block does not spell. A key
/// bound under two call types is left out (nothing to choose from).
#[derive(Debug, Default)]
pub(super) struct EventCallTypes {
    by_event: BTreeMap<(String, String), Option<&'static str>>,
    by_handler: BTreeMap<String, Option<&'static str>>,
}

impl EventCallTypes {
    fn insert(&mut self, event: String, handler: String, call_type: &'static str) {
        fn merge<K: Ord>(map: &mut BTreeMap<K, Option<&'static str>>, key: K, call_type: &'static str) {
            map.entry(key)
                .and_modify(|known| {
                    if *known != Some(call_type) {
                        *known = None;
                    }
                })
                .or_insert(Some(call_type));
        }
        merge(&mut self.by_handler, handler.clone(), call_type);
        merge(&mut self.by_event, (event, handler), call_type);
    }

    /// The call type of `handler` bound to the event `event` (its XML name).
    fn get(&self, event: &str, handler: &str) -> Option<&'static str> {
        match self.by_event.get(&(event.to_owned(), handler.to_owned())) {
            Some(known) => *known,
            None => self.by_handler.get(handler).copied().flatten(),
        }
    }

    #[cfg(test)]
    fn handler(handler: &str, call_type: &'static str) -> Self {
        let mut map = Self::default();
        map.by_handler.insert(handler.to_owned(), Some(call_type));
        map
    }
}

/// Call types over every event block of the form body text. One procedure
/// may intercept two events under two call types (fixture
/// `adopted/form_events_shared`: `Before` on OnOpen, `After` on BeforeClose),
/// so the event is part of the key.
pub(super) fn form_event_call_types(text: &str) -> EventCallTypes {
    let mut found = EventCallTypes::default();
    let bytes = text.as_bytes();
    for (start, byte) in bytes.iter().enumerate() {
        if *byte != b'{' {
            continue;
        }
        let Some(end) = scan_1c_braced_value(text, start) else {
            continue;
        };
        let Some(bindings) = parse_form_event_block(&text[start..end]) else {
            continue;
        };
        for binding in bindings {
            let identifier = binding.event.trim().trim_matches('"').to_owned();
            let event = form_event_name_from_identifier(&identifier)
                .map_or(identifier, str::to_owned);
            found.insert(event, binding.handler, CALL_TYPES[usize::from(binding.code)]);
        }
    }
    found
}

/// `xml` with `callType` on every `<Event>` whose handler the map knows.
pub(super) fn with_event_call_types(xml: &str, call_types: &EventCallTypes) -> String {
    let mut out = String::with_capacity(xml.len() + 64);
    let mut rest = xml;
    while let Some(at) = rest.find("<Event name=\"") {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let Some(open_end) = rest.find('>') else {
            break;
        };
        let Some(close) = rest.find("</Event>") else {
            break;
        };
        let open = &rest[..open_end];
        let handler = unescape_xml_text(&rest[open_end + 1..close]);
        let name_start = "<Event name=\"".len();
        let event = open[name_start..]
            .find('"')
            .map(|end| unescape_xml_text(&open[name_start..name_start + end]))
            .unwrap_or_default();
        match call_types.get(&event, &handler) {
            Some(call_type) if !open.contains("callType=") => {
                out.push_str(open);
                out.push_str(&format!(" callType=\"{call_type}\""));
            }
            _ => out.push_str(open),
        }
        rest = &rest[open_end..];
    }
    out.push_str(rest);
    out
}

fn unescape_xml_text(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
}

/// The base form body an adopted form carries, when it is one.
pub(super) fn adopted_base_form(body: &ParsedFormBodyBlob) -> Option<ParsedFormBodyBlob> {
    let flag = body.trailing.get(5)?.trim();
    let base = body.trailing.get(6)?.trim();
    if flag != "1" || !base.starts_with('{') {
        return None;
    }
    crate::module_blob::parse_form_body_plain(base).ok()
}

/// The adopted form's XML with its call types and `<BaseForm>`; any other
/// form unchanged. `object_refs` is passed for dialect 2.21, whose base form
/// goes through the same 8.5 pass as the form itself (8.5.1.1529 writes
/// `WindowOpeningMode`/`Group` inside `<BaseForm>` too; fixture
/// `v85_extension/adopted_form_events`).
pub(super) fn with_adopted_form_parts(
    xml: String,
    body: &ParsedFormBodyBlob,
    context: &FormParseContext<'_>,
    source_version: InfobaseConfigSourceVersion,
    object_refs: &BTreeMap<String, String>,
) -> Result<String> {
    let Some(base) = adopted_base_form(body) else {
        return Ok(xml);
    };
    let xml = with_event_call_types(&xml, &form_event_call_types(&body.layout));
    let v85 = source_version == InfobaseConfigSourceVersion::V2_21;
    let (base, facts) = if v85 && super::form::layout_8_5_1::is_form_body_8_5_1(&base) {
        let (converted, facts) = super::form::layout_8_5_1::down_convert_form_body_8_5_1(&base)
            .context("the adopted form's 8.5 base form")?;
        (converted, Some(facts))
    } else {
        (base, None)
    };
    let base_xml = match extract_form_body_xml_from_body_detailed_timed(&base, context, None) {
        Some(DetailedFormBodyExtraction::Emitted { xml, .. }) => xml,
        _ => anyhow::bail!("the adopted form's base form is not readable"),
    };
    let base_xml = match (v85, &facts) {
        (false, _) => base_xml,
        (true, Some(facts)) => {
            super::form::xml_2_21_writer::apply_form_facts_8_5_1(base_xml, facts, object_refs)?.0
        }
        (true, None) => super::form::xml_2_21_writer::apply_xml_2_21_upgrade_defaults(base_xml)?,
    };
    let base_xml = with_event_call_types(&base_xml, &form_event_call_types(&base.layout));
    let inner = base_form_inner(&base_xml).context("base form XML has no <Form> root")?;
    let close = xml.rfind("</Form>").context("form XML has no </Form>")?;
    let mut out = String::with_capacity(xml.len() + inner.len() + 64);
    out.push_str(&xml[..close]);
    if inner.is_empty() {
        out.push_str(&format!("\t<BaseForm version=\"{}\"/>\r\n", source_version.as_str()));
    } else {
        out.push_str(&format!("\t<BaseForm version=\"{}\">\r\n", source_version.as_str()));
        for line in inner.split_inclusive("\r\n") {
            out.push('\t');
            out.push_str(line);
        }
        out.push_str("\t</BaseForm>\r\n");
    }
    out.push_str(&xml[close..]);
    Ok(out)
}

/// The lines between `<Form …>` and `</Form>`.
fn base_form_inner(xml: &str) -> Option<&str> {
    let root = xml.find("<Form ")?;
    let open_end = root + xml[root..].find(">\r\n")? + 3;
    let close = xml.rfind("</Form>")?;
    xml.get(open_end..close)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_count_beyond_the_fields_is_no_event_block() {
        assert!(parse_form_event_block("{18446744073709551615,e,\"h\",1,0,e,0,1}").is_none());
    }

    #[test]
    fn reads_every_handler_of_an_interceptor_block() {
        let block = "{3,3ccc650e-f631-4cae-8e33-3eaac610b5f9,\"Перед\",52dbb775-1631-4fd5-8c55-1615b5881dac,\
                     \"Вместо\",9f2e5ddb-3492-4f5d-8f0d-416b8d1d5c5b,\"\",1,0,\
                     3ccc650e-f631-4cae-8e33-3eaac610b5f9,0,1,52dbb775-1631-4fd5-8c55-1615b5881dac,2,1,\
                     9f2e5ddb-3492-4f5d-8f0d-416b8d1d5c5b,0,2,\"После\",1}";
        let bindings = parse_form_event_block(block).unwrap();
        let spelled = bindings
            .iter()
            .map(|b| (b.handler.as_str(), CALL_TYPES[usize::from(b.code)]))
            .collect::<Vec<_>>();
        assert_eq!(
            spelled,
            vec![("Перед", "Before"), ("Вместо", "Override"), ("После", "After")]
        );
        assert_eq!(bindings[2].event, "9f2e5ddb-3492-4f5d-8f0d-416b8d1d5c5b");
    }

    #[test]
    fn a_configuration_block_reads_one_handler_per_event() {
        let block = "{1,9f2e5ddb-3492-4f5d-8f0d-416b8d1d5c5b,\"ПриСозданииНаСервере\",1,0,\
                     9f2e5ddb-3492-4f5d-8f0d-416b8d1d5c5b,0,1}";
        assert_eq!(parse_form_event_block(block).unwrap().len(), 1);
        assert!(parse_form_event_block("{1,x,\"h\",1,0,y,0,1}").is_none());
    }

    #[test]
    fn writes_the_call_type_of_a_known_handler() {
        let map = EventCallTypes::handler("Перед", "Before");
        assert_eq!(
            with_event_call_types("<Event name=\"OnOpen\">Перед</Event><Event name=\"x\">y</Event>", &map),
            "<Event name=\"OnOpen\" callType=\"Before\">Перед</Event><Event name=\"x\">y</Event>"
        );
    }
}
