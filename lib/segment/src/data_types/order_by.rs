use common::types::PointOffsetType;
use itertools::Either;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use validator::Validate;

use crate::id_tracker::IdTrackerSS;
use crate::types::{PointIdType, Range};

#[derive(Debug, Deserialize, Serialize, JsonSchema, Copy, Clone, Default)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    #[default]
    Asc,
    Desc,
}

impl Direction {
    pub fn as_range(&self, from: f64) -> Range {
        match self {
            Direction::Asc => Range {
                gte: Some(from),
                ..Default::default()
            },
            Direction::Desc => Range {
                lte: Some(from),
                ..Default::default()
            },
        }
    }
}

#[derive(Debug, Deserialize, Serialize, JsonSchema, Validate, Clone, Default)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "snake_case")]
pub struct OrderBy {
    /// Payload key to order by
    pub key: String,

    /// Direction of ordering: `asc` or `desc`. Default is ascending.
    pub direction: Option<Direction>,

    /// Used internally (not exposed to the user in scroll) to know which payload value to start scrolling from,
    /// we get this offset value by fetching the offset id in the request
    #[serde(skip)]
    pub value_offset: Option<f64>,
}

impl OrderBy {
    pub fn as_range(&self) -> Range {
        match self.value_offset {
            Some(offset) => self.direction.unwrap().as_range(offset),
            None => Range {
                ..Default::default()
            },
        }
    }

    pub fn direction(&self) -> Direction {
        self.direction.unwrap_or_default()
    }

    pub fn value_offset(&self) -> f64 {
        self.value_offset.unwrap_or_else(|| match self.direction() {
            Direction::Asc => f64::NEG_INFINITY,
            Direction::Desc => f64::INFINITY,
        })
    }
}

#[derive(Debug)]
pub(crate) struct OrderableItem {
    pub value: f64,
    pub external_id: PointIdType,
    pub internal_id: PointOffsetType,
}

/// Iterates over points ordered in (value, external_id) order
pub(crate) struct OrderedByFieldIterator<'a> {
    /// Iterator of points ordered only by their value
    stream: Box<dyn Iterator<Item = OrderableItem> + 'a>,

    /// Buffer used to keep same-value points ordered by their external id
    buffer: Vec<OrderableItem>,

    direction: Direction,

    /// Stores the first item of the next group of same-value points
    carry_over: Option<OrderableItem>,
}

impl<'a> OrderedByFieldIterator<'a> {
    pub fn new(
        range_iter: impl DoubleEndedIterator<Item = (f64, PointOffsetType)> + 'a,
        id_tracker: &'a IdTrackerSS,
        direction: Direction,
    ) -> Self {
        let stream = Box::new(
            match direction {
                Direction::Asc => Either::Left(range_iter),
                Direction::Desc => Either::Right(range_iter.rev()),
            }
            .filter_map(|(value, internal_id)| {
                id_tracker
                    .external_id(internal_id)
                    .map(|external_id| OrderableItem {
                        value,
                        external_id,
                        internal_id,
                    })
            }),
        );

        Self {
            stream,
            buffer: Vec::new(),
            direction,
            carry_over: None,
        }
    }
}

impl Iterator for OrderedByFieldIterator<'_> {
    type Item = OrderableItem;

    fn next(&mut self) -> Option<Self::Item> {
        // Every time the buffer is empty, we refill it with the next group of the same-value points,
        // ordered by their external id
        if self.buffer.is_empty() {
            let mut group = Vec::new();
            let first_item = self.carry_over.take().or_else(|| self.stream.next())?;

            for item in self.stream.by_ref() {
                if item.value != first_item.value {
                    self.carry_over = Some(item);
                    break;
                }
                group.push(item);
            }

            group.push(first_item);

            group.sort_unstable_by_key(|item| item.external_id);

            // The direction looks backwards here, but the buffer un-reverses the order when popping
            let iter = match self.direction {
                Direction::Asc => Either::Left(group.into_iter().rev()),
                Direction::Desc => Either::Right(group.into_iter()),
            };

            self.buffer.extend(iter);
        }

        self.buffer.pop()
    }
}
