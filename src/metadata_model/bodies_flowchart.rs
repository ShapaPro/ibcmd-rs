//! Business process flowcharts, base-free: `Ext/Flowchart.xml` -> the
//! `<uuid>.7` row.
//!
//! `{5,{<scheme>},N,<code>,<item>...,N}`: the scheme (background, grid, print
//! parameters), then each item as its kind code and record, in document
//! order, then the item count again. The grammar is the one the exporter
//! reads (`mssql_dump::parse_business_process_flowchart_text_with_types`);
//! what the XML does not spell -- the outline points of a shape, the kind
//! constants of each record, the true-branch mark of a line -- is measured
//! over the 22 flowcharts of the four corpora:
//!
//! - a shape's outline is computed from its `<Location>`: a rectangle for
//!   activities, processings and sub-processes; a pentagon for start and
//!   completion whose point rises `w/2·tan 30°`; a hexagon for a condition
//!   whose ends rise `h/2·tan 30°`; triangles for split and join;
//! - a line records whether it leaves a condition through its true port;
//! - the addressing attributes of an activity are ordered by uuid.
//!
//! Two stored flowcharts end with a larger count than their items: items
//! were deleted, which the source does not record.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

use super::DescriptorContext;
use super::brace::{Brace, NIL_UUID, parse_row};
use super::common::{crlf_strings, up_convert_v85_primitives, v85_layout};
use super::types::{design_time_ref, object_uuid};
use super::xml::{Element, parse_element_tree};
use crate::brace_list;
use crate::compiler::bodies::form_native::{format_native_color, format_native_font};

/// The border and line record's constant member.
const LINE_RECORD_UUID: &str = "e45c0cd8-a878-4bcb-8e1a-af934481e1cc";

/// `Ext/Flowchart.xml` next to the business process's XML.
pub fn flowchart_path(owner_xml: &Path) -> PathBuf {
    owner_xml
        .with_extension("")
        .join("Ext")
        .join("Flowchart.xml")
}

/// The `<uuid>.7` row, `None` when the process has no `Ext/Flowchart.xml`.
pub fn flowchart_row(owner_xml: &Path, context: &DescriptorContext) -> Result<Option<Brace>> {
    let path = flowchart_path(owner_xml);
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let schema = parse_element_tree(&bytes)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    let mut row = Flowchart::new(&schema, context)?.to_brace()?;
    crlf_strings(&mut row);
    if v85_layout(context) {
        up_convert_v85_primitives(&mut row);
    }
    Ok(Some(row))
}

struct Flowchart<'a> {
    schema: &'a Element,
    items: Vec<&'a Element>,
    context: &'a DescriptorContext,
}

fn text<'a>(element: &'a Element, name: &str) -> &'a str {
    element.child_text(name).unwrap_or_default()
}

fn number(text: &str, what: &str) -> Result<i64> {
    let text = text.trim();
    text.parse::<i64>()
        .with_context(|| format!("{what} is not a number: {text:?}"))
}

fn flag(element: &Element, name: &str) -> bool {
    text(element, name).trim() == "true"
}

fn code(table: &[(&str, i64)], value: &str, what: &str) -> Result<i64> {
    let value = value.trim();
    table
        .iter()
        .find_map(|(name, code)| (*name == value).then_some(*code))
        .ok_or_else(|| anyhow!("no flowchart code for {what} {value:?}"))
}

fn literal(text: &str) -> Result<Brace> {
    parse_row(text.as_bytes()).with_context(|| format!("bad brace literal {text:?}"))
}

/// `{1,<count>,{"lang","text"}...}`.
fn localized(element: Option<&Element>) -> Brace {
    let pairs = element.map(Element::localized).unwrap_or_default();
    let mut items = vec![Brace::num(1), Brace::num(pairs.len() as i64)];
    for (lang, content) in pairs {
        items.push(brace_list![Brace::str(lang), Brace::str(content)]);
    }
    Brace::List(items)
}

const DRAW_GRID_MODES: &[(&str, i64)] = &[("None", 0), ("Dots", 1), ("Lines", 3)];
const FIT_PAGE_MODES: &[(&str, i64)] = &[("Auto", 0)];
const HORIZONTAL_ALIGNS: &[(&str, i64)] = &[("Left", 0), ("Center", 1), ("Right", 2)];
const VERTICAL_ALIGNS: &[(&str, i64)] = &[("Top", 0), ("Center", 1), ("Bottom", 2)];
const PICTURE_LOCATIONS: &[(&str, i64)] = &[("Left", 1), ("Top", 2), ("Center", 5)];
const PICTURE_SIZES: &[(&str, i64)] = &[("RealSize", 0), ("Proportionally", 2), ("AutoSize", 4)];
const LINE_STYLES: &[(&str, i64)] = &[("None", 0), ("Solid", 1), ("Dashed", 2), ("Dotted", 3)];
const TEXT_LOCATIONS: &[(&str, i64)] = &[("FirstSegment", 0), ("Middle", 1)];
const ARROWS: &[(&str, i64)] = &[("None", 0), ("Filled", 1), ("Blank", 2)];
const SHAPES: &[(&str, i64)] = &[
    ("None", 0),
    ("Document", 10),
    ("Block", 11),
    ("HorizontalBrackets", 12),
];

impl<'a> Flowchart<'a> {
    fn new(schema: &'a Element, context: &'a DescriptorContext) -> Result<Self> {
        let items = schema
            .child("Items")
            .map(|items| items.children.iter().collect())
            .unwrap_or_default();
        Ok(Self {
            schema,
            items,
            context,
        })
    }

    fn to_brace(&self) -> Result<Brace> {
        let mut top = vec![
            Brace::num(5),
            brace_list![self.scheme()?],
            Brace::num(self.items.len() as i64),
        ];
        for item in &self.items {
            let (kind_code, body) = self.item(item)?;
            top.push(Brace::num(kind_code));
            top.push(body);
        }
        top.push(Brace::num(self.items.len() as i64));
        Ok(Brace::List(top))
    }

    /// `{1,<back>,<grid>,<h step>,<v step>,<grid mode>,6,6,{"N",top},7,
    /// {"N",left},8,{"N",bottom},9,{"N",right},13,{"N",b&w},16,{"N",fit}}`.
    fn scheme(&self) -> Result<Brace> {
        let schema = self.schema;
        let print = schema
            .child("PrintParameters")
            .ok_or_else(|| anyhow!("flowchart without <PrintParameters>"))?;
        let parameter = |key: i64, value: i64| {
            [
                Brace::num(key),
                brace_list![Brace::str("N"), Brace::num(value)],
            ]
        };
        let mut items = vec![
            Brace::num(1),
            self.color(text(schema, "BackColor"))?,
            Brace::flag(flag(schema, "GridEnabled")),
            Brace::num(number(
                text(schema, "GridHorizontalStep"),
                "GridHorizontalStep",
            )?),
            Brace::num(number(
                text(schema, "GridVerticalStep"),
                "GridVerticalStep",
            )?),
            Brace::num(code(
                DRAW_GRID_MODES,
                text(schema, "DrawGridMode"),
                "DrawGridMode",
            )?),
            Brace::num(6),
        ];
        items.extend(parameter(6, number(text(print, "TopMargin"), "TopMargin")?));
        items.extend(parameter(
            7,
            number(text(print, "LeftMargin"), "LeftMargin")?,
        ));
        items.extend(parameter(
            8,
            number(text(print, "BottomMargin"), "BottomMargin")?,
        ));
        items.extend(parameter(
            9,
            number(text(print, "RightMargin"), "RightMargin")?,
        ));
        items.extend(parameter(13, i64::from(flag(print, "BlackAndWhite"))));
        items.extend(parameter(
            16,
            code(FIT_PAGE_MODES, text(print, "FitPageMode"), "FitPageMode")?,
        ));
        Ok(Brace::List(items))
    }

    /// A colour: `auto` is the unset `{3,4,{0}}`, the rest as a form body
    /// stores it.
    fn color(&self, spelled: &str) -> Result<Brace> {
        let spelled = spelled.trim();
        if spelled == "auto" || spelled.is_empty() {
            return literal("{3,4,{0}}");
        }
        let native = format_native_color(Some(spelled), |name| {
            self.context
                .index
                .uuid_of(&format!("StyleItem.{name}"))
                .map(str::to_string)
        })
        .ok_or_else(|| anyhow!("unsupported flowchart colour {spelled:?}"))?;
        literal(&native)
    }

    fn font(&self, element: Option<&Element>) -> Result<Brace> {
        let attributes = element
            .map(|font| {
                font.attrs
                    .iter()
                    .cloned()
                    .collect::<std::collections::BTreeMap<_, _>>()
            })
            .unwrap_or_default();
        let native = format_native_font(&attributes, |name| {
            self.context
                .index
                .uuid_of(&format!("StyleItem.{name}"))
                .map(str::to_string)
        })
        .ok_or_else(|| anyhow!("unsupported flowchart font {attributes:?}"))?;
        literal(&native)
    }

    /// `{7,<back>,<line>,<text>,<font>,<tooltip>,<h align>,<v align>,
    /// <picture location>,<z order>,<hyperlink>,<transparent>,0,0}`.
    fn style(&self, properties: &Element) -> Result<Brace> {
        Ok(brace_list![
            Brace::num(7),
            self.color(text(properties, "BackColor"))?,
            self.color(text(properties, "LineColor"))?,
            self.color(text(properties, "TextColor"))?,
            self.font(properties.child("Font"))?,
            localized(properties.child("ToolTip")),
            Brace::num(code(
                HORIZONTAL_ALIGNS,
                text(properties, "HorizontalAlign"),
                "HorizontalAlign"
            )?),
            Brace::num(code(
                VERTICAL_ALIGNS,
                text(properties, "VerticalAlign"),
                "VerticalAlign"
            )?),
            Brace::num(code(
                PICTURE_LOCATIONS,
                text(properties, "PictureLocation"),
                "PictureLocation"
            )?),
            Brace::num(number(text(properties, "ZOrder"), "ZOrder")?),
            Brace::flag(flag(properties, "Hyperlink")),
            Brace::flag(flag(properties, "Transparent")),
            Brace::num(0),
            Brace::num(0),
        ])
    }

    /// `{4,0,{0},<style>,<width>,<gap>,e45c0cd8-...,0}`: a shape's border or a
    /// line's line.
    fn line_record(&self, element: Option<&Element>) -> Result<Brace> {
        let (style, width, gap) = match element {
            Some(element) => (
                code(
                    LINE_STYLES,
                    element.child_text("style").unwrap_or("Solid"),
                    "line style",
                )?,
                number(element.attr("width").unwrap_or("1"), "line width")?,
                element.attr("gap") == Some("true"),
            ),
            None => (1, 1, false),
        };
        Ok(brace_list![
            Brace::num(4),
            Brace::num(0),
            brace_list![Brace::num(0)],
            Brace::num(style),
            Brace::num(width),
            Brace::flag(gap),
            Brace::atom(LINE_RECORD_UUID),
            Brace::num(0),
        ])
    }

    /// The nine-member picture record: empty, or a platform picture by uuid.
    fn picture(&self, element: Option<&Element>) -> Result<Brace> {
        let reference = element
            .and_then(|picture| picture.child_text("Ref"))
            .map(str::trim)
            .unwrap_or_default();
        if element.and_then(|picture| picture.child("Abs")).is_some() {
            bail!("an inline flowchart picture is not supported");
        }
        if reference.is_empty() {
            return literal("{4,0,{0},\"\",-1,-1,1,0,\"\"}");
        }
        let load_transparent = element
            .and_then(|picture| picture.child_text("LoadTransparent"))
            .map(str::trim)
            != Some("false");
        let uuid = crate::mssql_dump::standard_picture_uuid(reference)
            .ok_or_else(|| anyhow!("unsupported flowchart picture {reference}"))?;
        Ok(brace_list![
            Brace::num(4),
            Brace::num(1),
            brace_list![Brace::num(0), Brace::uuid(uuid)],
            Brace::str(""),
            Brace::num(-1),
            Brace::num(-1),
            Brace::flag(load_transparent),
            Brace::num(0),
            Brace::str(""),
        ])
    }

    fn location(properties: &Element) -> Result<(i64, i64, i64, i64)> {
        let location = properties
            .child("Location")
            .ok_or_else(|| anyhow!("flowchart shape without <Location>"))?;
        let value = |name: &str| number(location.attr(name).unwrap_or("0"), name);
        Ok((
            value("left")?,
            value("top")?,
            value("right")?,
            value("bottom")?,
        ))
    }

    /// `{<style>,5,l,t,r,b,<n>,<points>...,<picture size>,<picture>,<border>}`.
    fn shape_geometry(&self, tag: &str, properties: &Element) -> Result<Brace> {
        let (left, top, right, bottom) = Self::location(properties)?;
        let points = outline(tag, left, top, right, bottom)?;
        let mut items = vec![
            self.style(properties)?,
            Brace::num(5),
            Brace::num(left),
            Brace::num(top),
            Brace::num(right),
            Brace::num(bottom),
            Brace::num(points.len() as i64),
        ];
        for (x, y) in points {
            items.push(Brace::num(x));
            items.push(Brace::num(y));
        }
        items.push(Brace::num(code(
            PICTURE_SIZES,
            text(properties, "PictureSize"),
            "PictureSize",
        )?));
        items.push(self.picture(properties.child("Picture"))?);
        items.push(self.line_record(properties.child("Border"))?);
        Ok(Brace::List(items))
    }

    /// `{<code>,<record>}` of one item.
    fn item(&self, item: &Element) -> Result<(i64, Brace)> {
        let properties = item
            .child("Properties")
            .ok_or_else(|| anyhow!("flowchart item without <Properties>"))?;
        let id = number(item.attr("id").unwrap_or_default(), "item id")?;
        let base = brace_list![
            Brace::num(4),
            Brace::num(id),
            localized(properties.child("Title")),
            Brace::str(text(properties, "Name")),
            Brace::num(number(text(properties, "TabOrder"), "TabOrder")?),
        ];
        let head = || -> Result<Brace> {
            let uuid = item
                .attr("uuid")
                .ok_or_else(|| anyhow!("flowchart {} without uuid", item.name))?;
            Ok(brace_list![
                base.clone(),
                Brace::num(4),
                Brace::uuid(uuid),
                Brace::num(0)
            ])
        };
        let tag = item.name.as_str();
        let shape = |tail: Vec<Brace>| -> Result<Brace> {
            let mut wrapper = vec![self.shape_geometry(tag, properties)?];
            wrapper.extend(tail);
            Ok(brace_list![Brace::List(wrapper)])
        };
        Ok(match tag {
            "Decoration" => (
                0,
                brace_list![base.clone(), Brace::num(2), self.decoration(properties)?],
            ),
            "ConnectionLine" => (1, self.line(base.clone(), properties)?),
            "Start" => (
                2,
                brace_list![
                    head()?,
                    Brace::num(2),
                    shape(vec![Brace::num(1)])?,
                    events(item, &["BeforeStart"])
                ],
            ),
            "Completion" => (
                3,
                brace_list![
                    head()?,
                    Brace::num(2),
                    shape(vec![Brace::num(1)])?,
                    events(item, &["OnComplete"])
                ],
            ),
            "Condition" => (
                4,
                brace_list![
                    head()?,
                    Brace::num(1),
                    shape(vec![
                        Brace::num(3),
                        Brace::num(number(text(properties, "TruePortIndex"), "TruePortIndex")?),
                        Brace::num(number(
                            text(properties, "FalsePortIndex"),
                            "FalsePortIndex"
                        )?),
                    ])?,
                    events(item, &["ConditionCheck"])
                ],
            ),
            "Activity" => (
                5,
                brace_list![
                    head()?,
                    Brace::num(8),
                    shape(vec![Brace::num(3), Brace::num(2), Brace::num(0)])?,
                    Brace::str(text(properties, "Explanation")),
                    Brace::flag(flag(properties, "Group")),
                    events(
                        item,
                        &[
                            "InteractiveActivationProcessing",
                            "BeforeCreateTasks",
                            "OnCreateTask",
                            "OnExecute",
                            "CheckExecutionProcessing",
                            "BeforeExecute",
                            "BeforeExecuteInteractively",
                        ]
                    ),
                    self.addressing_attributes(properties.child("AddressingAttributes"))?,
                    Brace::str(text(properties, "TaskDescription")),
                ],
            ),
            "Split" => (
                7,
                brace_list![head()?, Brace::num(1), shape(vec![Brace::num(1)])?],
            ),
            "Join" => (
                8,
                brace_list![head()?, Brace::num(1), shape(vec![Brace::num(1)])?],
            ),
            "Processing" => (
                9,
                brace_list![
                    head()?,
                    Brace::num(0),
                    shape(vec![Brace::num(1)])?,
                    events(item, &["Processing"])
                ],
            ),
            "SubBusinessProcess" => {
                let subprocess = text(properties, "Subprocess").trim();
                let subprocess = if subprocess.is_empty() {
                    NIL_UUID.to_string()
                } else {
                    object_uuid(subprocess, self.context)?
                };
                (
                    10,
                    brace_list![
                        head()?,
                        Brace::num(1),
                        shape(vec![Brace::num(1)])?,
                        events(
                            item,
                            &[
                                "BeforeCreateTasks",
                                "OnCreateTask",
                                "OnCreateSubBusinessProcesses",
                                "OnExecute",
                                "BeforeExecute",
                                "BeforeCreateSubBusinessProcesses",
                            ]
                        ),
                        Brace::uuid(&subprocess),
                        Brace::str(text(properties, "TaskDescription")),
                    ],
                )
            }
            other => bail!("unsupported flowchart item {other}"),
        })
    }

    /// `{{<style>,6,l,t,r,b,<picture size>,<picture>,<transparent>,<shape>,0,0}}`.
    fn decoration(&self, properties: &Element) -> Result<Brace> {
        let (left, top, right, bottom) = Self::location(properties)?;
        Ok(brace_list![brace_list![
            self.style(properties)?,
            Brace::num(6),
            Brace::num(left),
            Brace::num(top),
            Brace::num(right),
            Brace::num(bottom),
            Brace::num(code(
                PICTURE_SIZES,
                text(properties, "PictureSize"),
                "PictureSize"
            )?),
            self.picture(properties.child("Picture"))?,
            Brace::flag(flag(properties, "Transparent")),
            Brace::num(code(SHAPES, text(properties, "Shape"), "Shape")?),
            Brace::num(0),
            Brace::num(0),
        ]])
    }

    /// `{<base>,3,<from id>,<true branch>,<to id>,<decorative>,{{<style>,6,
    /// <n>,<points>...,<line>,<text location>,<from port>,<to port>,<m>,
    /// <segments>...,<begin arrow>,<end arrow>}}}`.
    fn line(&self, base: Brace, properties: &Element) -> Result<Brace> {
        let connect = properties.child("Connect");
        let end = |name: &str| -> Result<(i64, i64, String)> {
            let end = connect.and_then(|connect| connect.child(name));
            let item = end.map(|end| text(end, "Item").trim()).unwrap_or_default();
            let port = number(
                end.map(|end| text(end, "PortIndex")).unwrap_or("0"),
                "PortIndex",
            )?;
            let id = if item.is_empty() {
                -1
            } else {
                self.item_id(item)?
            };
            Ok((id, port, item.to_string()))
        };
        let (from_id, from_port, from_item) = end("From")?;
        let (to_id, to_port, _) = end("To")?;

        let mut geometry = vec![self.style(properties)?, Brace::num(6)];
        let points = properties
            .child("PivotPoints")
            .map(|points| points.children_named("Point").collect::<Vec<_>>())
            .unwrap_or_default();
        geometry.push(Brace::num(points.len() as i64));
        for point in points {
            geometry.push(Brace::num(number(point.attr("x").unwrap_or("0"), "x")?));
            geometry.push(Brace::num(number(point.attr("y").unwrap_or("0"), "y")?));
        }
        geometry.push(self.line_record(properties.child("Line"))?);
        geometry.push(Brace::num(code(
            TEXT_LOCATIONS,
            text(properties, "TextLocation"),
            "TextLocation",
        )?));
        geometry.push(Brace::num(from_port));
        geometry.push(Brace::num(to_port));
        let segments = properties
            .child("ManualyMovedSegments")
            .map(|segments| segments.children_named("Segment").collect::<Vec<_>>())
            .unwrap_or_default();
        geometry.push(Brace::num(segments.len() as i64));
        for segment in segments {
            for (name, axis) in [("Start", "x"), ("Start", "y"), ("End", "x"), ("End", "y")] {
                let point = segment
                    .child(name)
                    .ok_or_else(|| anyhow!("segment without <{name}>"))?;
                geometry.push(Brace::num(number(point.attr(axis).unwrap_or("0"), axis)?));
            }
            geometry.push(Brace::num(number(
                segment.attr("index").unwrap_or("0"),
                "segment index",
            )?));
        }
        geometry.push(Brace::num(code(
            ARROWS,
            text(properties, "BeginArrow"),
            "BeginArrow",
        )?));
        geometry.push(Brace::num(code(
            ARROWS,
            text(properties, "EndArrow"),
            "EndArrow",
        )?));

        Ok(brace_list![
            base,
            Brace::num(3),
            Brace::num(from_id),
            Brace::flag(self.is_true_branch(&from_item, from_port)),
            Brace::num(to_id),
            Brace::flag(flag(properties, "DecorativeLine")),
            brace_list![Brace::List(geometry)],
        ])
    }

    /// The id of the item a line names.
    fn item_id(&self, name: &str) -> Result<i64> {
        let item = self
            .items
            .iter()
            .find(|item| {
                item.child("Properties")
                    .and_then(|properties| properties.child_text("Name"))
                    == Some(name)
            })
            .ok_or_else(|| anyhow!("a flowchart line names no item {name:?}"))?;
        number(item.attr("id").unwrap_or_default(), "item id")
    }

    /// Whether a line leaves a condition through its true port.
    fn is_true_branch(&self, from_item: &str, from_port: i64) -> bool {
        self.items.iter().any(|item| {
            item.name == "Condition"
                && item.child("Properties").is_some_and(|properties| {
                    properties.child_text("Name") == Some(from_item)
                        && text(properties, "TruePortIndex").trim() == from_port.to_string()
                })
        })
    }

    /// `{<count>,{<attribute uuid>,<value>}...}`, ordered by uuid.
    fn addressing_attributes(&self, list: Option<&Element>) -> Result<Brace> {
        let mut attributes = Vec::new();
        for attribute in list
            .into_iter()
            .flat_map(|list| list.children_named("AddressingAttribute"))
        {
            let reference = attribute.attr("ref").unwrap_or_default();
            let uuid = object_uuid(reference, self.context)?;
            let value = match attribute.child("Value") {
                None => brace_list![Brace::str("U")],
                Some(value) if value.is_nil() => brace_list![Brace::str("U")],
                Some(value) => design_time_ref(&value.text, self.context)?,
            };
            attributes.push((uuid, value));
        }
        attributes.sort_by(|left, right| left.0.cmp(&right.0));
        let mut items = vec![Brace::num(attributes.len() as i64)];
        for (uuid, value) in attributes {
            items.push(brace_list![Brace::uuid(&uuid), value]);
        }
        Ok(Brace::List(items))
    }
}

/// `{<count>,{<index>,"<handler>"}...}`: the events that name a handler, by
/// their index in the kind's event list.
fn events(item: &Element, names: &[&str]) -> Brace {
    let mut handlers = Vec::new();
    for event in item
        .child("Events")
        .into_iter()
        .flat_map(|events| events.children_named("Event"))
    {
        let handler = event.text.trim();
        if handler.is_empty() {
            continue;
        }
        let Some(index) = names
            .iter()
            .position(|name| Some(*name) == event.attr("name"))
        else {
            continue;
        };
        handlers.push((index, handler.to_string()));
    }
    handlers.sort_by_key(|(index, _)| *index);
    let mut items = vec![Brace::num(handlers.len() as i64)];
    for (index, handler) in handlers {
        items.push(brace_list![Brace::num(index as i64), Brace::str(handler)]);
    }
    Brace::List(items)
}

/// `h/2·tan 30°` (or `w/2·tan 30°`), truncated: how far a pointed shape's
/// point rises.
fn rise(extent: i64) -> i64 {
    ((extent as f64) * (30f64.to_radians().tan()) / 2.0) as i64
}

/// A shape's outline as the platform stores it, from its location.
fn outline(tag: &str, left: i64, top: i64, right: i64, bottom: i64) -> Result<Vec<(i64, i64)>> {
    let (w, h) = (right - left, bottom - top);
    let (r, b) = (right - 1, bottom - 1);
    Ok(match tag {
        "Activity" | "Processing" | "SubBusinessProcess" => {
            vec![(left, top), (r, top), (r, b), (left, b)]
        }
        "Start" => {
            let shoulder = bottom - rise(w);
            vec![
                (left, top),
                (r, top),
                (r, shoulder),
                (left + w / 2, b),
                (left, shoulder),
            ]
        }
        "Completion" => {
            let shoulder = top + rise(w);
            vec![
                (left + w / 2, top),
                (r, shoulder),
                (r, b),
                (left, b),
                (left, shoulder),
            ]
        }
        "Condition" => {
            let inset = rise(h);
            let middle = top + h / 2;
            vec![
                (left, middle),
                (left + inset, top),
                (r - inset, top),
                (r, middle),
                (r - inset, b),
                (left + inset, b),
            ]
        }
        "Split" => vec![(left, top), (right - 2, top), (left + (w - 2) / 2, b)],
        "Join" => vec![(left, b), (right - 2, b), (left + (w - 2) / 2, top)],
        other => bail!("no outline for flowchart {other}"),
    })
}

#[cfg(test)]
mod tests {
    use super::outline;

    #[test]
    fn computes_the_outlines_the_platform_stores() {
        // BSP `Задание` and ERP УХ flowcharts, point for point.
        assert_eq!(
            outline("Start", 380, 20, 420, 60).unwrap(),
            vec![(380, 20), (419, 20), (419, 49), (400, 59), (380, 49)]
        );
        assert_eq!(
            outline("Completion", 180, 460, 220, 500).unwrap(),
            vec![(200, 460), (219, 471), (219, 499), (180, 499), (180, 471)]
        );
        assert_eq!(
            outline("Condition", 140, 240, 260, 300).unwrap(),
            vec![
                (140, 270),
                (157, 240),
                (242, 240),
                (259, 270),
                (242, 299),
                (157, 299)
            ]
        );
        assert_eq!(
            outline("Split", 320, 160, 360, 180).unwrap(),
            vec![(320, 160), (358, 160), (339, 179)]
        );
        assert_eq!(
            outline("Join", 320, 400, 360, 420).unwrap(),
            vec![(320, 419), (358, 419), (339, 400)]
        );
    }
}
