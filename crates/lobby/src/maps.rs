//! Catálogo y gestión de mapas de Counter-Strike 2 para el veto y selección.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MapInfo {
    pub id: String,
    pub name: String,
    pub image: String,
    pub icon: String,
    pub active: bool,
    #[serde(rename = "isOfficialPremier")]
    pub is_official_premier: bool,
}

pub fn default_catalog() -> Vec<MapInfo> {
    vec![
        MapInfo {
            id: "de_mirage".into(),
            name: "Mirage".into(),
            image: "https://raw.githubusercontent.com/ggMartinez/CS2-Maps-Images/main/maps/de_mirage.png".into(),
            icon: "https://raw.githubusercontent.com/MurkyYT/cs2-map-icons/main/images/de_mirage.png".into(),
            active: true,
            is_official_premier: true,
        },
        MapInfo {
            id: "de_inferno".into(),
            name: "Inferno".into(),
            image: "https://raw.githubusercontent.com/ggMartinez/CS2-Maps-Images/main/maps/de_inferno.png".into(),
            icon: "https://raw.githubusercontent.com/MurkyYT/cs2-map-icons/main/images/de_inferno.png".into(),
            active: true,
            is_official_premier: true,
        },
        MapInfo {
            id: "de_dust2".into(),
            name: "Dust II".into(),
            image: "https://raw.githubusercontent.com/ggMartinez/CS2-Maps-Images/main/maps/de_dust2.png".into(),
            icon: "https://raw.githubusercontent.com/MurkyYT/cs2-map-icons/main/images/de_dust2.png".into(),
            active: true,
            is_official_premier: true,
        },
        MapInfo {
            id: "de_nuke".into(),
            name: "Nuke".into(),
            image: "https://raw.githubusercontent.com/ggMartinez/CS2-Maps-Images/main/maps/de_nuke.png".into(),
            icon: "https://raw.githubusercontent.com/MurkyYT/cs2-map-icons/main/images/de_nuke.png".into(),
            active: true,
            is_official_premier: true,
        },
        MapInfo {
            id: "de_ancient".into(),
            name: "Ancient".into(),
            image: "https://raw.githubusercontent.com/ggMartinez/CS2-Maps-Images/main/maps/de_ancient.png".into(),
            icon: "https://raw.githubusercontent.com/MurkyYT/cs2-map-icons/main/images/de_ancient.png".into(),
            active: true,
            is_official_premier: true,
        },
        MapInfo {
            id: "de_anubis".into(),
            name: "Anubis".into(),
            image: "https://raw.githubusercontent.com/ggMartinez/CS2-Maps-Images/main/maps/de_anubis.png".into(),
            icon: "https://raw.githubusercontent.com/MurkyYT/cs2-map-icons/main/images/de_anubis.png".into(),
            active: true,
            is_official_premier: true,
        },
        MapInfo {
            id: "de_vertigo".into(),
            name: "Vertigo".into(),
            image: "https://raw.githubusercontent.com/ggMartinez/CS2-Maps-Images/main/maps/de_vertigo.png".into(),
            icon: "https://raw.githubusercontent.com/MurkyYT/cs2-map-icons/main/images/de_vertigo.png".into(),
            active: true,
            is_official_premier: true,
        },
        MapInfo {
            id: "de_train".into(),
            name: "Train".into(),
            image: "https://raw.githubusercontent.com/ggMartinez/CS2-Maps-Images/main/maps/de_train.png".into(),
            icon: "https://raw.githubusercontent.com/MurkyYT/cs2-map-icons/main/images/de_train.png".into(),
            active: false,
            is_official_premier: false,
        },
        MapInfo {
            id: "de_overpass".into(),
            name: "Overpass".into(),
            image: "https://raw.githubusercontent.com/ggMartinez/CS2-Maps-Images/main/maps/de_overpass.png".into(),
            icon: "https://raw.githubusercontent.com/MurkyYT/cs2-map-icons/main/images/de_overpass.png".into(),
            active: false,
            is_official_premier: false,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_catalog_has_7_premier_maps() {
        let catalog = default_catalog();
        let premier_count = catalog.iter().filter(|m| m.is_official_premier && m.active).count();
        assert_eq!(premier_count, 7);
    }
}
