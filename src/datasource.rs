use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use crate::{
    config::{Chat, Location},
    snafu,
    whatever::WhateverSync,
};

/// Provides an efficient way to perform lookups on the configuration data.
pub struct Datasource {
    location_by_station_id: HashMap<String, Arc<Location>>,
    locations_by_chat_id: HashMap<i64, Vec<Arc<Location>>>,
}

impl Datasource {
    /// Validates the input data and creates a new Datasource.
    pub fn new(
        locations: impl IntoIterator<Item = Location>,
        chats: impl IntoIterator<Item = Chat>,
    ) -> Result<Self, WhateverSync> {
        let mut location_by_id = HashMap::<String, Arc<Location>>::new();
        let mut location_by_station_id = HashMap::<String, Arc<Location>>::new();
        let mut park_ids = HashSet::<String>::new();

        for location in locations {
            let location = Arc::new(location);
            if location_by_id
                .insert(location.id.clone(), location.clone())
                .is_some()
            {
                return Err(snafu!("found duplicate location id: {}", &location.id));
            }

            for park in &location.parks {
                if !park_ids.insert(park.id.clone()) {
                    return Err(snafu!("found duplicate park id: {}", &park.id));
                }

                for station in &park.stations {
                    if location_by_station_id
                        .insert(station.id.clone(), location.clone())
                        .is_some()
                    {
                        return Err(snafu!("found duplicate station id: {}", &park.id));
                    }
                }
            }
        }

        let mut locations_by_chat_id = HashMap::<i64, Vec<Arc<Location>>>::new();
        for chat in chats {
            let chat = Arc::new(chat);
            let mut chat_locations = Vec::<Arc<Location>>::with_capacity(chat.locations.len());
            for location_id in &chat.locations {
                match location_by_id.get(location_id) {
                    Some(loc) => chat_locations.push(loc.clone()),
                    None => {
                        return Err(snafu!(
                            "chat {} references unknown location id: {}",
                            chat.id,
                            location_id
                        ))
                    }
                }
            }

            if locations_by_chat_id
                .insert(chat.id, chat_locations)
                .is_some()
            {
                return Err(snafu!("found duplicate chat id: {}", &chat.id));
            }
        }

        Ok(Self {
            location_by_station_id,
            locations_by_chat_id,
        })
    }

    /// Returns a Location that contains a station with the given ID.
    pub fn get_location_by_station(&self, id: &str) -> Option<&Location> {
        self.location_by_station_id.get(id).map(|loc| loc.as_ref())
    }

    /// Returns a list of Locations accessible from the Chat with the given ID.
    pub fn get_locations_by_chat(&self, id: i64) -> Box<dyn Iterator<Item = &Location> + '_> {
        self.locations_by_chat_id
            .get(&id)
            .map(|locs| {
                Box::new(locs.iter().map(|loc| loc.as_ref())) as Box<dyn Iterator<Item = &Location>>
            })
            .unwrap_or_else(|| Box::new(std::iter::empty()))
    }
}

#[cfg(test)]
mod tests {
    use super::Datasource;
    use crate::config::{Chat, Location, Park, Station};

    #[test]
    fn empty() {
        let ds = Datasource::new(vec![], vec![]).unwrap();
        assert!(ds.get_location_by_station("x").is_none());
        assert_eq!(ds.get_locations_by_chat(123).count(), 0);
    }

    #[test]
    fn basic() {
        let locations = vec![
            Location {
                id: "l1".to_string(),
                name: "L1".to_string(),
                parks: vec![
                    Park {
                        id: "l1p1".to_string(),
                        stations: vec![
                            Station {
                                id: "l1s1".to_string(),
                                alias: "l1S1".to_string(),
                            },
                            Station {
                                id: "l1s2".to_string(),
                                alias: "l1S2".to_string(),
                            },
                        ],
                    },
                    Park {
                        id: "l1p2".to_string(),
                        stations: vec![
                            Station {
                                id: "l1s3".to_string(),
                                alias: "l1S3".to_string(),
                            },
                            Station {
                                id: "l1s4".to_string(),
                                alias: "l1S4".to_string(),
                            },
                        ],
                    },
                ],
            },
            Location {
                id: "l2".to_string(),
                name: "L2".to_string(),
                parks: vec![Park {
                    id: "l2p1".to_string(),
                    stations: vec![Station {
                        id: "l2s1".to_string(),
                        alias: "l2S1".to_string(),
                    }],
                }],
            },
        ];

        let chats = vec![
            Chat {
                id: 1,
                locations: vec!["l1".to_string()],
            },
            Chat {
                id: 2,
                locations: vec!["l2".to_string()],
            },
            Chat {
                id: 3,
                locations: vec!["l1".to_string(), "l2".to_string()],
            },
        ];

        let ds = Datasource::new(locations, chats).unwrap();

        assert!(ds.get_location_by_station("l3").is_none()); // location is not defined

        assert_eq!(ds.get_location_by_station("l1s4").unwrap().name, "L1");
        assert_eq!(ds.get_location_by_station("l2s1").unwrap().name, "L2");

        assert_eq!(ds.get_locations_by_chat(10).count(), 0); // chat is not defined

        let result = ds.get_locations_by_chat(2).collect::<Vec<&Location>>();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "L2");

        let result = ds.get_locations_by_chat(3).collect::<Vec<&Location>>();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].name, "L1");
        assert_eq!(result[1].name, "L2");
    }
}
