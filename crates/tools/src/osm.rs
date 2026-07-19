//! OpenStreetMap (Nominatim) geocoding and reverse-geocoding tools.
//! Uses the free Nominatim API — no API key required. The usage policy
//! mandates at most 1 request per second and a descriptive User-Agent header.

use std::time::{Duration, Instant};

use reqwest::{Client, Url};
use serde_json::{json, Value};
use tokio::sync::Mutex;

const MIN_INTERVAL: Duration = Duration::from_secs(1);
const MAX_SEARCH_LIMIT: usize = 5;

/// Rate-limited client for the Nominatim API.
pub struct OsmClient {
    client: Client,
    last_request: Mutex<Option<Instant>>,
}

impl Default for OsmClient {
    fn default() -> Self {
        Self {
            client: Client::builder()
                .user_agent("housebot/1.0 (Discord assistant bot; OSM integration)")
                .timeout(Duration::from_secs(15))
                .build()
                .expect("OSM HTTP client should build"),
            last_request: Mutex::new(None),
        }
    }
}

impl OsmClient {
    /// Enforce the 1-request-per-second Nominatim policy.
    /// Returns a guard held for the caller's request lifetime so that the
    /// ordering is preserved even if a task is stalled before `.send().await`.
    async fn rate_limit(&self) -> tokio::sync::MutexGuard<'_, Option<Instant>> {
        let mut last = self.last_request.lock().await;
        if let Some(t) = *last {
            let elapsed = t.elapsed();
            if elapsed < MIN_INTERVAL {
                tokio::time::sleep(MIN_INTERVAL - elapsed).await;
            }
        }
        *last = Some(Instant::now());
        last
    }

    /// Search for a location by name (forward geocoding).
    pub async fn search_location(&self, query: &str, limit: usize) -> String {
        let _guard = self.rate_limit().await;
        let limit = limit.clamp(1, MAX_SEARCH_LIMIT);
        let limit_str = limit.to_string();
        let url = Url::parse_with_params(
            "https://nominatim.openstreetmap.org/search",
            &[
                ("q", query),
                ("format", "json"),
                ("limit", limit_str.as_str()),
                ("addressdetails", "1"),
            ],
        )
        .expect("valid Nominatim search URL");
        match self.client.get(url).send().await {
            Ok(response) if response.status().is_success() => {
                let results: Vec<Value> = response.json().await.unwrap_or_default();
                if results.is_empty() {
                    return format!("No results found for '{query}'.");
                }
                let lines: Vec<String> = results.iter().map(format_place).collect();
                lines.join("\n\n")
            }
            Ok(response) => format!("Error: Nominatim returned HTTP {}", response.status()),
            Err(e) => format!("Error: could not reach Nominatim: {e}"),
        }
    }

    /// Reverse geocode coordinates to an address.
    pub async fn lookup_coordinates(&self, lat: f64, lon: f64) -> String {
        let _guard = self.rate_limit().await;
        let lat_str = lat.to_string();
        let lon_str = lon.to_string();
        let url = Url::parse_with_params(
            "https://nominatim.openstreetmap.org/reverse",
            &[
                ("lat", lat_str.as_str()),
                ("lon", lon_str.as_str()),
                ("format", "json"),
                ("addressdetails", "1"),
            ],
        )
        .expect("valid Nominatim reverse URL");
        match self.client.get(url).send().await {
            Ok(response) if response.status().is_success() => {
                let data: Value = response.json().await.unwrap_or_default();
                let display_name = data["display_name"].as_str().unwrap_or("Unknown location");
                let lat = data["lat"].as_str().unwrap_or("?");
                let lon = data["lon"].as_str().unwrap_or("?");
                format!("{display_name}\nCoordinates: {lat}, {lon}")
            }
            Ok(response) => format!("Error: Nominatim returned HTTP {}", response.status()),
            Err(e) => format!("Error: could not reach Nominatim: {e}"),
        }
    }
}

fn format_place(place: &Value) -> String {
    let name = place["display_name"].as_str().unwrap_or("Unknown");
    let lat = place["lat"].as_str().unwrap_or("?");
    let lon = place["lon"].as_str().unwrap_or("?");
    let osm_type = place["osm_type"].as_str().unwrap_or("");
    let category = place["category"].as_str().unwrap_or("");
    let type_ = place["type"].as_str().unwrap_or("");
    let extras = if !category.is_empty() || !type_.is_empty() {
        format!(" ({category}/{type_})")
    } else {
        String::new()
    };
    format!("- **{name}**{extras}\n  OSM type: {osm_type}  |  Lat: {lat}  |  Lon: {lon}")
}

/// Tool definition: search_location (forward geocoding).
pub fn search_definition() -> Value {
    json!({
        "name": "search_location",
        "description": "Search for a location or place by name using OpenStreetMap (Nominatim). \
            Returns coordinates, place type, and address details. \
            Free API, no key required.",
        "input_schema": {
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The location name to search for (e.g. 'Eiffel Tower, Paris' or 'Tokyo, Japan')."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 5,
                    "default": 3
                }
            },
            "required": ["query"]
        }
    })
}

/// Tool definition: lookup_coordinates (reverse geocoding).
pub fn lookup_definition() -> Value {
    json!({
        "name": "lookup_coordinates",
        "description": "Reverse-geocode latitude/longitude coordinates to a \
            human-readable address using OpenStreetMap (Nominatim). \
            Free API, no key required.",
        "input_schema": {
            "type": "object",
            "properties": {
                "latitude": {
                    "type": "number",
                    "description": "Latitude (e.g. 48.8584)."
                },
                "longitude": {
                    "type": "number",
                    "description": "Longitude (e.g. 2.2945)."
                }
            },
            "required": ["latitude", "longitude"]
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_definition_has_required_fields() {
        let d = search_definition();
        assert_eq!(d["name"], "search_location");
        assert!(d["input_schema"]["properties"].get("query").is_some());
        assert_eq!(d["input_schema"]["required"], json!(["query"]));
    }

    #[test]
    fn lookup_definition_has_required_fields() {
        let d = lookup_definition();
        assert_eq!(d["name"], "lookup_coordinates");
        assert!(d["input_schema"]["properties"].get("latitude").is_some());
        assert!(d["input_schema"]["properties"].get("longitude").is_some());
        assert_eq!(
            d["input_schema"]["required"],
            json!(["latitude", "longitude"])
        );
    }

    #[test]
    fn format_place_renders_minimal_fields() {
        let place = json!({
            "display_name": "Paris, France",
            "lat": "48.8566",
            "lon": "2.3522",
            "osm_type": "relation",
            "category": "place",
            "type": "city"
        });
        let out = format_place(&place);
        assert!(out.contains("Paris, France"));
        assert!(out.contains("Lat: 48.8566"));
        assert!(out.contains("Lon: 2.3522"));
        assert!(out.contains("place/city"));
    }

    #[test]
    fn format_place_handles_missing_type() {
        let place = json!({
            "display_name": "Somewhere",
            "lat": "0.0",
            "lon": "0.0",
            "osm_type": "node"
        });
        let out = format_place(&place);
        assert!(out.contains("Somewhere"));
        assert!(!out.contains("/"));
    }
}
