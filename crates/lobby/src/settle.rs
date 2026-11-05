//! Módulo de liquidación y cierre de partidas Mixton con reparto de premios, reembolsos e historial.

use std::sync::Arc;

/// Acción a ejecutar tras finalizar o cancelar una partida.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchAction {
    WinA,
    WinB,
    Cancel,
}

impl MatchAction {
    /// Determina la acción a partir del ganador reportado o de los marcadores.
    ///
    /// Sigue la lógica del legado (`cs2Api.js:138-140`):
    /// - "team1" o "faction1" -> `WinA`
    /// - "team2" o "faction2" -> `WinB`
    /// - Si no: si score_a > score_b -> `WinA`, si score_b > score_a -> `WinB`, empate o ausente -> `WinA`.
    pub fn from_winner(
        winner: Option<&str>,
        score_a: Option<i64>,
        score_b: Option<i64>,
    ) -> MatchAction {
        match winner {
            Some("team1") | Some("faction1") => MatchAction::WinA,
            Some("team2") | Some("faction2") => MatchAction::WinB,
            _ => {
                let a = score_a.unwrap_or(0);
                let b = score_b.unwrap_or(0);
                if a > b {
                    MatchAction::WinA
                } else if b > a {
                    MatchAction::WinB
                } else {
                    MatchAction::WinA
                }
            }
        }
    }

    /// Representación en texto compatible con el backend legado.
    pub fn as_str(&self) -> &'static str {
        match self {
            MatchAction::WinA => "win_a",
            MatchAction::WinB => "win_b",
            MatchAction::Cancel => "cancel",
        }
    }
}

/// Pago o reembolso individual a acreditar a un jugador por Discord ID.
#[derive(Debug, Clone, PartialEq)]
pub struct Payout {
    pub discord_id: String,
    pub amount: f64,
    pub reason: String,
}

/// Extrae un número en punto flotante ya sea que venga como Number o String.
fn get_f64(val: Option<&serde_json::Value>) -> Option<f64> {
    match val {
        Some(serde_json::Value::Number(n)) => n.as_f64(),
        Some(serde_json::Value::String(s)) => s.parse::<f64>().ok(),
        _ => None,
    }
}

/// Calcula los pagos de premios o reembolsos derivados del fin o cancelación de la partida.
///
/// Replica `resolveMixtonMatchEnd` paso 3:
/// - `bet = max(0, bet_amount_per_player)`
/// - `total_pot = doc.total_pot o bet * 10`
/// - Solo si `bet > 0`:
///   - `WinA` -> cada jugador de `faction1_roster` con `discord_id` recibe `round2(total_pot / len(faction1_roster))`
///     con reason `🏆 Premio ganador partida mixton {match_id} (S/ {premio})`.
///   - `WinB` -> análogo con `faction2_roster`.
///   - `Cancel` -> cada jugador de ambos rosters que NO esté en `manual_paid_players` recibe `bet`
///     con reason `↩️ Reembolso por cancelación de partida mixton {match_id}`.
/// - Jugadores sin `discord_id` son ignorados.
pub fn compute_payouts(
    doc: &serde_json::Value,
    action: MatchAction,
    match_id: &str,
) -> Vec<Payout> {
    let bet = get_f64(doc.get("bet_amount_per_player"))
        .unwrap_or(0.0)
        .max(0.0);
    if bet <= 0.0 {
        return Vec::new();
    }

    let total_pot = get_f64(doc.get("total_pot")).unwrap_or(bet * 10.0);

    match action {
        MatchAction::WinA => {
            let roster = match doc.get("faction1_roster").and_then(|v| v.as_array()) {
                Some(arr) if !arr.is_empty() => arr,
                _ => return Vec::new(),
            };
            let premio = data::round2(total_pot / (roster.len() as f64));
            let mut payouts = Vec::new();
            for p in roster {
                if let Some(discord_id) = p
                    .get("discord_id")
                    .or_else(|| p.get("discordId"))
                    .and_then(|v| v.as_str())
                {
                    let d = discord_id.trim();
                    if !d.is_empty() {
                        payouts.push(Payout {
                            discord_id: d.to_string(),
                            amount: premio,
                            reason: format!(
                                "🏆 Premio ganador partida mixton {match_id} (S/ {premio})"
                            ),
                        });
                    }
                }
            }
            payouts
        }
        MatchAction::WinB => {
            let roster = match doc.get("faction2_roster").and_then(|v| v.as_array()) {
                Some(arr) if !arr.is_empty() => arr,
                _ => return Vec::new(),
            };
            let premio = data::round2(total_pot / (roster.len() as f64));
            let mut payouts = Vec::new();
            for p in roster {
                if let Some(discord_id) = p
                    .get("discord_id")
                    .or_else(|| p.get("discordId"))
                    .and_then(|v| v.as_str())
                {
                    let d = discord_id.trim();
                    if !d.is_empty() {
                        payouts.push(Payout {
                            discord_id: d.to_string(),
                            amount: premio,
                            reason: format!(
                                "🏆 Premio ganador partida mixton {match_id} (S/ {premio})"
                            ),
                        });
                    }
                }
            }
            payouts
        }
        MatchAction::Cancel => {
            let manual_paid_set: std::collections::HashSet<String> = doc
                .get("manual_paid_players")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|x| x.as_str().map(|s| s.trim().to_string()))
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default();

            let roster_a = doc.get("faction1_roster").and_then(|v| v.as_array());
            let roster_b = doc.get("faction2_roster").and_then(|v| v.as_array());

            let mut all_players = Vec::new();
            if let Some(ra) = roster_a {
                all_players.extend(ra.iter());
            }
            if let Some(rb) = roster_b {
                all_players.extend(rb.iter());
            }

            let mut payouts = Vec::new();
            for p in all_players {
                if let Some(discord_id) = p
                    .get("discord_id")
                    .or_else(|| p.get("discordId"))
                    .and_then(|v| v.as_str())
                {
                    let d = discord_id.trim();
                    if !d.is_empty() && !manual_paid_set.contains(d) {
                        payouts.push(Payout {
                            discord_id: d.to_string(),
                            amount: bet,
                            reason: format!(
                                "↩️ Reembolso por cancelación de partida mixton {match_id}"
                            ),
                        });
                    }
                }
            }
            payouts
        }
    }
}

/// Limpia y normaliza el roster de una facción para que cada elemento contenga {id, nickname, discord_id}.
fn clean_roster(val: Option<&serde_json::Value>) -> Vec<serde_json::Value> {
    val.and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|p| {
                    let id = p.get("id").and_then(|v| v.as_str()).unwrap_or("");
                    let nickname = p.get("nickname").and_then(|v| v.as_str()).unwrap_or("");
                    let discord_id = p
                        .get("discord_id")
                        .or_else(|| p.get("discordId"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    serde_json::json!({
                        "id": id,
                        "nickname": nickname,
                        "discord_id": discord_id,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Construye el documento de historial replicando `addMatchToHistory` (`database.js:1607-1660`).
pub fn build_history_doc(
    doc: &serde_json::Value,
    match_id: &str,
    action: MatchAction,
    score_a: Option<i64>,
    score_b: Option<i64>,
    stats: Option<&serde_json::Value>,
) -> serde_json::Value {
    let raw_games = doc
        .get("games_per_match")
        .or_else(|| doc.get("rounds_per_game"))
        .and_then(|v| {
            v.as_i64()
                .or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok()))
        })
        .unwrap_or(1);
    let games_count = if raw_games == 1 || raw_games == 3 || raw_games == 5 {
        raw_games
    } else {
        1
    };

    let default_win_score = if games_count <= 1 {
        1
    } else {
        games_count / 2 + 1
    };

    let (final_score_a, final_score_b, winner, status) = match action {
        MatchAction::WinA => (
            score_a.unwrap_or(default_win_score),
            score_b.unwrap_or(0),
            "faction1",
            "finished",
        ),
        MatchAction::WinB => (
            score_a.unwrap_or(0),
            score_b.unwrap_or(default_win_score),
            "faction2",
            "finished",
        ),
        MatchAction::Cancel => (
            score_a.unwrap_or(0),
            score_b.unwrap_or(0),
            "CANCELLED",
            "cancelled",
        ),
    };

    let map_name = doc
        .get("map")
        .and_then(|v| v.as_str())
        .or_else(|| {
            doc.get("chosen_map")
                .and_then(|m| m.get("name"))
                .and_then(|v| v.as_str())
        })
        .unwrap_or("Manual");

    let bet = get_f64(doc.get("bet_amount_per_player"))
        .unwrap_or(0.0)
        .max(0.0);
    let total_pot = get_f64(doc.get("total_pot")).unwrap_or(bet * 10.0);

    let entity_custom_name = doc
        .get("entity_custom_name")
        .and_then(|v| v.as_str())
        .unwrap_or("Mixton");
    let game = doc.get("game").and_then(|v| v.as_str()).unwrap_or("cs2");
    let faction1_name = doc
        .get("faction1_name")
        .and_then(|v| v.as_str())
        .unwrap_or("Equipo 1");
    let faction2_name = doc
        .get("faction2_name")
        .and_then(|v| v.as_str())
        .unwrap_or("Equipo 2");
    let queue_id = doc
        .get("queue_id")
        .and_then(|v| v.as_str())
        .unwrap_or("steam");
    let queue_name = doc
        .get("queue_name")
        .and_then(|v| v.as_str())
        .unwrap_or("Lobby privado de Steam");
    let launch_mode = doc
        .get("launch_mode")
        .and_then(|v| v.as_str())
        .unwrap_or("steam");
    let guild_id = doc
        .get("guild_id")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let tournament_match_id = doc
        .get("tournament_match_id")
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    let mut obj = serde_json::json!({
        "id": match_id,
        "entity_custom_name": entity_custom_name,
        "game": game,
        "map": map_name,
        "location": "Mixton",
        "faction1_name": faction1_name,
        "faction2_name": faction2_name,
        "faction1_score": final_score_a,
        "faction2_score": final_score_b,
        "winner": winner,
        "status": status,
        "queue_id": queue_id,
        "queue_name": queue_name,
        "faction1_roster": clean_roster(doc.get("faction1_roster")),
        "faction2_roster": clean_roster(doc.get("faction2_roster")),
        "guild_id": guild_id,
        "launch_mode": launch_mode,
        "bet_amount_per_player": bet,
        "total_pot": total_pot,
        "games_per_match": games_count,
        "rounds_per_game": games_count,
        "tournament_match_id": tournament_match_id,
        "updatedAt": data::timestamp_now(),
    });

    if let Some(s) = stats {
        if !s.is_null() {
            if let serde_json::Value::Object(ref mut map) = obj {
                map.insert("stats".to_string(), s.clone());
            }
        }
    }

    obj
}

/// Dependencias requeridas para la liquidación de partidas.
pub struct SettleDeps {
    pub cs2: Option<Arc<cs2::Cs2ServerManager>>,
    pub docs: Arc<dyn data::DocSink>,
    pub source: Arc<dyn data::DocSource>,
    pub wallet: Option<Arc<dyn data::WalletStore>>,
}

/// Error que describe la imposibilidad de resolver la partida.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ResolveError {
    #[error("partida no encontrada o ya resuelta")]
    NotFound,
    #[error("no autorizado para liquidar partida con apuesta")]
    Unauthorized,
    #[error("error de persistencia: {0}")]
    Store(String),
}

/// Resultado de la resolución de una partida.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolveOutcome {
    pub match_id: String,
    pub action: MatchAction,
    pub winner: String,
    pub payouts_applied: usize,
    pub payout_failures: Vec<String>,
}

/// Resuelve el cierre de una partida aplicando reclamo atómico, pagos e historial.
pub async fn resolve_match_end(
    deps: &SettleDeps,
    match_id: &str,
    action: MatchAction,
    score_a: Option<i64>,
    score_b: Option<i64>,
    authenticated: bool,
    stats: Option<serde_json::Value>,
) -> Result<ResolveOutcome, ResolveError> {
    // 1) source.get("faceit_active_matches", match_id) -> None -> Err(NotFound) (y aun así cs2.clear_match)
    let active_doc = match deps.source.get("faceit_active_matches", match_id).await {
        Ok(Some(doc)) => doc,
        Ok(None) => {
            if let Some(ref cs2) = deps.cs2 {
                cs2.clear_match(match_id).await;
            }
            return Err(ResolveError::NotFound);
        }
        Err(e) => return Err(ResolveError::Store(e.to_string())),
    };

    // 2) si bet > 0 y !authenticated -> Err(Unauthorized) SIN reclamar el documento
    let bet = get_f64(active_doc.get("bet_amount_per_player"))
        .unwrap_or(0.0)
        .max(0.0);
    if bet > 0.0 && !authenticated {
        return Err(ResolveError::Unauthorized);
    }

    // 3) docs.take_doc(...) (reclamo atómico; si devuelve None otro cierre ya lo tomó -> Err(NotFound))
    let claimed_doc = match deps.docs.take_doc("faceit_active_matches", match_id).await {
        Ok(Some(doc)) => doc,
        Ok(None) => return Err(ResolveError::NotFound),
        Err(e) => return Err(ResolveError::Store(e.to_string())),
    };

    // 4) cs2.clear_match(match_id) si hay cs2
    if let Some(ref cs2) = deps.cs2 {
        cs2.clear_match(match_id).await;
    }

    // 5) pagos: por cada Payout wallet.adjust(guild_id del doc, discord_id, amount, reason)
    let payouts = compute_payouts(&claimed_doc, action, match_id);
    let guild_id = claimed_doc.get("guild_id").and_then(|v| v.as_str());

    let mut payouts_applied = 0;
    let mut payout_failures = Vec::new();

    if !payouts.is_empty() {
        if let Some(ref wallet) = deps.wallet {
            for p in payouts {
                match wallet
                    .adjust(guild_id, &p.discord_id, p.amount, &p.reason)
                    .await
                {
                    Ok(_) => payouts_applied += 1,
                    Err(err) => {
                        let msg = format!("fallo al pagar a {}: {}", p.discord_id, err);
                        tracing::error!(
                            match_id,
                            discord_id = %p.discord_id,
                            amount = p.amount,
                            %err,
                            "Error al acreditar pago de partida"
                        );
                        payout_failures.push(msg);
                    }
                }
            }
        } else {
            tracing::error!(
                match_id,
                "WalletStore no configurado pero hay pagos pendientes por aplicar"
            );
            for p in payouts {
                payout_failures.push(format!("WalletStore ausente para jugador {}", p.discord_id));
            }
        }
    }

    // 6) docs.set_doc("faceit_matches_history", match_id, build_history_doc(..))
    // (merge si ya existe: usa merge_doc si set_doc reemplazaría createdAt; añade createdAt solo si el doc no existía, consultando source.get antes)
    let mut history_doc = build_history_doc(
        &claimed_doc,
        match_id,
        action,
        score_a,
        score_b,
        stats.as_ref(),
    );

    // Los pagos que fallaron quedan registrados en el historial para poder conciliarlos a mano:
    // el documento activo ya se reclamó (borró) y no habrá segundo intento automático.
    if !payout_failures.is_empty() {
        if let serde_json::Value::Object(ref mut map) = history_doc {
            map.insert(
                "payout_failures".to_string(),
                serde_json::json!(payout_failures),
            );
        }
    }

    // Tras pagar, un fallo al escribir el historial NO aborta el cierre (los pagos ya se hicieron):
    // se registra y se informa en `payout_failures`.
    let history_result: Result<(), String> = async {
        let existing_history = deps
            .source
            .get("faceit_matches_history", match_id)
            .await
            .map_err(|e| e.to_string())?;

        if existing_history.is_none() {
            let mut doc_with_created = history_doc.clone();
            if let serde_json::Value::Object(ref mut map) = doc_with_created {
                map.insert("createdAt".to_string(), data::timestamp_now());
            }
            deps.docs
                .set_doc("faceit_matches_history", match_id, &doc_with_created)
                .await
                .map_err(|e| e.to_string())
        } else {
            deps.docs
                .merge_doc("faceit_matches_history", match_id, &history_doc)
                .await
                .map_err(|e| e.to_string())
        }
    }
    .await;

    if let Err(err) = history_result {
        tracing::error!(match_id, %err, "Error al guardar el historial de la partida");
        payout_failures.push(format!("historial no guardado: {err}"));
    }

    // 7) si cs2 -> tokio::spawn(cs2.auto_shutdown_if_idle())
    if let Some(ref cs2) = deps.cs2 {
        let cs2_clone = Arc::clone(cs2);
        tokio::spawn(async move {
            let _ = cs2_clone.auto_shutdown_if_idle().await;
        });
    }

    let winner = match action {
        MatchAction::WinA => "faction1".to_string(),
        MatchAction::WinB => "faction2".to_string(),
        MatchAction::Cancel => "CANCELLED".to_string(),
    };

    Ok(ResolveOutcome {
        match_id: match_id.to_string(),
        action,
        winner,
        payouts_applied,
        payout_failures,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use data::WalletStore;
    use serde_json::json;

    #[test]
    fn test_match_action_from_winner() {
        // Por winner explícito
        assert_eq!(
            MatchAction::from_winner(Some("team1"), None, None),
            MatchAction::WinA
        );
        assert_eq!(
            MatchAction::from_winner(Some("faction1"), None, None),
            MatchAction::WinA
        );
        assert_eq!(
            MatchAction::from_winner(Some("team2"), None, None),
            MatchAction::WinB
        );
        assert_eq!(
            MatchAction::from_winner(Some("faction2"), None, None),
            MatchAction::WinB
        );

        // Sin winner pero con marcadores
        assert_eq!(
            MatchAction::from_winner(None, Some(13), Some(11)),
            MatchAction::WinA
        );
        assert_eq!(
            MatchAction::from_winner(None, Some(9), Some(13)),
            MatchAction::WinB
        );
        assert_eq!(
            MatchAction::from_winner(None, Some(12), Some(12)),
            MatchAction::WinA
        );
        assert_eq!(
            MatchAction::from_winner(None, None, None),
            MatchAction::WinA
        );
    }

    #[test]
    fn test_compute_payouts_win_a_and_win_b() {
        let doc = json!({
            "bet_amount_per_player": 10.0,
            "total_pot": 100.0,
            "faction1_roster": [
                {"id": "p1", "nickname": "A1", "discord_id": "d1"},
                {"id": "p2", "nickname": "A2", "discord_id": "d2"},
                {"id": "p3", "nickname": "A3", "discord_id": "d3"},
                {"id": "p4", "nickname": "A4", "discord_id": "d4"},
                {"id": "p5", "nickname": "A5", "discord_id": "d5"},
            ],
            "faction2_roster": [
                {"id": "p6", "nickname": "B1", "discord_id": "d6"},
                {"id": "p7", "nickname": "B2", "discord_id": "d7"},
            ]
        });

        // WinA: 100 / 5 = 20.0 por jugador
        let payouts_a = compute_payouts(&doc, MatchAction::WinA, "m1");
        assert_eq!(payouts_a.len(), 5);
        assert_eq!(payouts_a[0].amount, 20.0);
        assert_eq!(payouts_a[0].discord_id, "d1");
        assert!(payouts_a[0]
            .reason
            .contains("🏆 Premio ganador partida mixton m1 (S/ 20)"));

        // WinB: 100 / 2 = 50.0 por jugador
        let payouts_b = compute_payouts(&doc, MatchAction::WinB, "m1");
        assert_eq!(payouts_b.len(), 2);
        assert_eq!(payouts_b[0].amount, 50.0);
        assert_eq!(payouts_b[0].discord_id, "d6");
        assert_eq!(payouts_b[1].discord_id, "d7");
    }

    #[test]
    fn test_compute_payouts_cancel_with_manual_paid() {
        let doc = json!({
            "bet_amount_per_player": 10.0,
            "manual_paid_players": ["d2", "d6"],
            "faction1_roster": [
                {"discord_id": "d1"},
                {"discord_id": "d2"},
                {"discord_id": ""},
            ],
            "faction2_roster": [
                {"discord_id": "d5"},
                {"discord_id": "d6"},
                {"nickname": "sin_discord"},
            ]
        });

        // Cancel: d1 y d5 reciben reembolso (10.0); d2 y d6 excluidos por manual_paid; vacíos excluidos
        let payouts = compute_payouts(&doc, MatchAction::Cancel, "m_cancel");
        assert_eq!(payouts.len(), 2);
        assert_eq!(payouts[0].discord_id, "d1");
        assert_eq!(payouts[0].amount, 10.0);
        assert!(payouts[0]
            .reason
            .contains("↩️ Reembolso por cancelación de partida mixton m_cancel"));
        assert_eq!(payouts[1].discord_id, "d5");
        assert_eq!(payouts[1].amount, 10.0);
    }

    #[test]
    fn test_compute_payouts_zero_bet_returns_empty() {
        let doc = json!({
            "bet_amount_per_player": 0.0,
            "faction1_roster": [{"discord_id": "d1"}],
        });
        assert!(compute_payouts(&doc, MatchAction::WinA, "m_free").is_empty());
        assert!(compute_payouts(&doc, MatchAction::Cancel, "m_free").is_empty());
    }

    #[test]
    fn test_build_history_doc_structure() {
        let doc = json!({
            "entity_custom_name": "Torneo Mixton",
            "map": "de_inferno",
            "faction1_name": "Los T",
            "faction2_name": "Los CT",
            "games_per_match": 3,
            "bet_amount_per_player": 15.0,
            "total_pot": 150.0,
            "guild_id": "g123",
            "faction1_roster": [
                {"id": "p1", "nickname": "T1", "discord_id": "d1"}
            ],
            "faction2_roster": [
                {"id": "p2", "nickname": "CT1", "discord_id": "d2"}
            ]
        });

        let hist = build_history_doc(
            &doc,
            "m999",
            MatchAction::WinA,
            None,
            None,
            Some(&json!({"mvp": "T1"})),
        );

        assert_eq!(hist["id"], "m999");
        assert_eq!(hist["map"], "de_inferno");
        assert_eq!(hist["winner"], "faction1");
        assert_eq!(hist["status"], "finished");
        // defaultWinScore para bo3 es 3/2 + 1 = 2
        assert_eq!(hist["faction1_score"], 2);
        assert_eq!(hist["faction2_score"], 0);
        assert_eq!(hist["games_per_match"], 3);
        assert_eq!(hist["rounds_per_game"], 3);
        assert_eq!(hist["bet_amount_per_player"], 15.0);
        assert_eq!(hist["total_pot"], 150.0);
        assert_eq!(hist["stats"]["mvp"], "T1");
        assert!(hist.get("updatedAt").is_some());
    }

    #[tokio::test]
    async fn test_pago_fallido_no_aborta_y_queda_en_el_historial() {
        let sink = Arc::new(data::FakeDocSink::new());
        sink.insert(
            "faceit_active_matches",
            "m_fallo",
            json!({
                "guild_id": "g1",
                "bet_amount_per_player": 10.0,
                "total_pot": 20.0,
                "faction1_roster": [{"id": "p1", "nickname": "A1", "discord_id": "d1"}],
                "faction2_roster": [{"id": "p2", "nickname": "B1", "discord_id": "d2"}],
            }),
        );
        // Sin monedero: el pago no se puede aplicar, pero el cierre debe completarse.
        let deps = SettleDeps {
            cs2: None,
            docs: sink.clone(),
            source: sink.clone(),
            wallet: None,
        };

        let res = resolve_match_end(
            &deps,
            "m_fallo",
            MatchAction::WinA,
            Some(1),
            Some(0),
            true,
            None,
        )
        .await
        .expect("el cierre no debe abortar por un pago fallido");

        assert_eq!(res.payouts_applied, 0);
        assert_eq!(res.payout_failures.len(), 1);
        assert!(sink.get("faceit_active_matches", "m_fallo").is_none());

        let hist = sink
            .get("faceit_matches_history", "m_fallo")
            .expect("historial escrito");
        assert_eq!(hist["status"], "finished");
        assert_eq!(hist["payout_failures"].as_array().map(|a| a.len()), Some(1));
    }

    #[tokio::test]
    async fn test_resolve_match_end_lifecycle_with_fakes() {
        let sink = Arc::new(data::FakeDocSink::new());
        let wallet = Arc::new(data::FakeWalletStore::new());

        // Configuramos saldo inicial de 90 a dos jugadores
        wallet.set_balance(Some("g1"), "d1", 90.0);
        wallet.set_balance(Some("g1"), "d2", 90.0);

        let active_match = json!({
            "guild_id": "g1",
            "bet_amount_per_player": 10.0,
            "total_pot": 20.0,
            "faction1_roster": [{"id": "p1", "nickname": "A1", "discord_id": "d1"}],
            "faction2_roster": [{"id": "p2", "nickname": "B1", "discord_id": "d2"}],
        });

        sink.insert("faceit_active_matches", "m_res_1", active_match);

        let deps = SettleDeps {
            cs2: None,
            docs: sink.clone(),
            source: sink.clone(),
            wallet: Some(wallet.clone()),
        };

        // 1. Intentar liquidar sin autenticar cuando hay apuesta -> Unauthorized
        let unauth = resolve_match_end(
            &deps,
            "m_res_1",
            MatchAction::WinA,
            Some(13),
            Some(5),
            false,
            None,
        )
        .await;
        assert_eq!(unauth, Err(ResolveError::Unauthorized));
        // Documento no fue tomado
        assert!(sink.get("faceit_active_matches", "m_res_1").is_some());

        // 2. Liquidar autenticado -> Ok
        let res = resolve_match_end(
            &deps,
            "m_res_1",
            MatchAction::WinA,
            Some(13),
            Some(5),
            true,
            None,
        )
        .await
        .expect("resolución exitosa");

        assert_eq!(res.payouts_applied, 1);
        assert_eq!(res.winner, "faction1");

        // Activa ya no existe
        assert_eq!(sink.get("faceit_active_matches", "m_res_1"), None);

        // Historial fue creado y contiene createdAt y updatedAt
        let hist = sink
            .get("faceit_matches_history", "m_res_1")
            .expect("historial existe");
        assert_eq!(hist["winner"], "faction1");
        assert_eq!(hist["faction1_score"], 13);
        assert_eq!(hist["faction2_score"], 5);
        assert!(hist.get("createdAt").is_some());
        assert!(hist.get("updatedAt").is_some());

        // Saldo de d1 pasó de 90 a 110 (90 + 20 premio)
        let bal_d1 = wallet.balance(Some("g1"), "d1").await.unwrap();
        assert_eq!(bal_d1, 110.0);
        // Saldo de d2 no cambió (90)
        let bal_d2 = wallet.balance(Some("g1"), "d2").await.unwrap();
        assert_eq!(bal_d2, 90.0);

        // 3. Segunda llamada -> NotFound
        let not_found = resolve_match_end(
            &deps,
            "m_res_1",
            MatchAction::WinA,
            Some(13),
            Some(5),
            true,
            None,
        )
        .await;
        assert_eq!(not_found, Err(ResolveError::NotFound));
    }
}
