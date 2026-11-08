-- ============================================================================
-- MACHUPEEK - ESQUEMA RELACIONAL COMPLETO PARA MYSQL 8.0+
-- ============================================================================
-- Motor: InnoDB | Charset: utf8mb4 | Collation: utf8mb4_unicode_ci
-- Reemplaza el modelo documental de Firestore manteniendo 100% de paridad
-- funcional con los crates: data, web, lobby, bot y cs2.
-- ============================================================================

SET NAMES utf8mb4;
SET time_zone = '+00:00';
SET FOREIGN_KEY_CHECKS = 0;

CREATE DATABASE IF NOT EXISTS machupeek
  CHARACTER SET utf8mb4
  COLLATE utf8mb4_unicode_ci;

USE machupeek;

-- ============================================================================
-- 0. MOTOR DE TIEMPO REAL Y DOCUMENTOS (CDC / PUB-SUB EQUIVALENTE A ONSNAPSHOT)
--    Proporciona paridad 100% con Firestore (watch_collection / watch_document)
--    mediante eventos secuenciales (realtime_events) y triggers automáticos.
-- ============================================================================

CREATE TABLE IF NOT EXISTS realtime_documents (
    collection      VARCHAR(64) NOT NULL COMMENT 'Nombre de la colección (faceit_users, guild_settings, faceit_config, etc.)',
    doc_id          VARCHAR(128) NOT NULL COMMENT 'ID del documento dentro de la colección',
    doc_json        JSON NOT NULL COMMENT 'Contenido JSON íntegro del documento (compatible con Node/Rust)',
    created_at      DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    updated_at      DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3) ON UPDATE CURRENT_TIMESTAMP(3),

    PRIMARY KEY (collection, doc_id),
    KEY idx_rt_docs_updated (collection, updated_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE IF NOT EXISTS realtime_events (
    seq             BIGINT UNSIGNED NOT NULL AUTO_INCREMENT COMMENT 'Secuencia monótona para streams en tiempo real',
    collection      VARCHAR(64) NOT NULL,
    doc_id          VARCHAR(128) NOT NULL,
    op              ENUM('UPSERT', 'DELETE') NOT NULL,
    changed_at      DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),

    PRIMARY KEY (seq),
    KEY idx_rt_events_col_seq (collection, seq),
    KEY idx_rt_events_changed_at (changed_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

DROP TRIGGER IF EXISTS trg_rt_docs_after_insert;
DROP TRIGGER IF EXISTS trg_rt_docs_after_update;
DROP TRIGGER IF EXISTS trg_rt_docs_after_delete;

DELIMITER $$

CREATE TRIGGER trg_rt_docs_after_insert
AFTER INSERT ON realtime_documents
FOR EACH ROW
BEGIN
    INSERT INTO realtime_events (collection, doc_id, op)
    VALUES (NEW.collection, NEW.doc_id, 'UPSERT');
END$$

CREATE TRIGGER trg_rt_docs_after_update
AFTER UPDATE ON realtime_documents
FOR EACH ROW
BEGIN
    INSERT INTO realtime_events (collection, doc_id, op)
    VALUES (NEW.collection, NEW.doc_id, 'UPSERT');
END$$

CREATE TRIGGER trg_rt_docs_after_delete
AFTER DELETE ON realtime_documents
FOR EACH ROW
BEGIN
    INSERT INTO realtime_events (collection, doc_id, op)
    VALUES (OLD.collection, OLD.doc_id, 'DELETE');
END$$

DELIMITER ;

-- ============================================================================
-- 1. SERVIDORES DE DISCORD (GUILDS) Y CONFIGURACIÓN
--    Equivale a: faceit_config/settings, guild_settings/{guildId},
--                faceit_config/messages, faceit_config/panel_*
-- ============================================================================

CREATE TABLE IF NOT EXISTS guilds (
    guild_id                        VARCHAR(32) NOT NULL COMMENT 'ID del servidor de Discord o GLOBAL para ajustes por defecto',
    name                            VARCHAR(128) NOT NULL DEFAULT 'Servidor MachuPeek',
    icon_url                        VARCHAR(512) NULL,
    member_count                    INT UNSIGNED NOT NULL DEFAULT 0,
    is_default                      BOOLEAN NOT NULL DEFAULT FALSE COMMENT 'True para el servidor principal (settings.discordGuildId)',

    -- Canales de Discord
    discord_link_channel_id         VARCHAR(32) NULL,
    discord_matches_channel_id      VARCHAR(32) NULL,
    discord_mixton_channel_id       VARCHAR(32) NULL,
    discord_mixton_chat_channel_id  VARCHAR(32) NULL,
    discord_roles_channel_id        VARCHAR(32) NULL,
    discord_status_channel_id       VARCHAR(32) NULL,
    discord_waiting_room_channel_id VARCHAR(32) NULL,
    discord_admin_alerts_channel_id VARCHAR(32) NULL,
    discord_sanctions_channel_id    VARCHAR(32) NULL,
    discord_support_channel_id      VARCHAR(32) NULL,
    discord_music_channel_id        VARCHAR(32) NULL,
    discord_recharge_channel_id     VARCHAR(32) NULL,

    -- Roles de Discord
    discord_match_access_role_id    VARCHAR(32) NULL,
    hub_role_id                     VARCHAR(32) NULL,
    discord_hub_role_id             VARCHAR(32) NULL,
    admin_role_id                   VARCHAR(64) NULL,
    admin_role_name                 VARCHAR(64) NULL,

    -- Integración FACEIT
    faceit_club_id                  VARCHAR(64) NULL,
    faceit_hub_id                   VARCHAR(64) NULL,
    faceit_leaderboard_id           VARCHAR(64) NULL,

    -- Reglas de partida
    match_timeout_minutes           INT UNSIGNED NOT NULL DEFAULT 120,
    games_per_match                 TINYINT UNSIGNED NOT NULL DEFAULT 1 COMMENT 'Valores permitidos: 1, 3 o 5 (BO1, BO3, BO5)',
    rounds_per_game                 TINYINT UNSIGNED NOT NULL DEFAULT 1 COMMENT 'Alias legado de games_per_match',

    -- Campos adicionales dinámicos (extra)
    extra_settings                  JSON NULL,

    created_at                      DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    updated_at                      DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3) ON UPDATE CURRENT_TIMESTAMP(3),

    PRIMARY KEY (guild_id),
    KEY idx_guilds_is_default (is_default),
    CONSTRAINT chk_guilds_games_per_match CHECK (games_per_match IN (1, 3, 5))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- Interruptores de módulos y comandos (Feature Flags globales y por servidor)
CREATE TABLE IF NOT EXISTS guild_feature_flags (
    guild_id                        VARCHAR(32) NOT NULL COMMENT 'FK a guilds.guild_id (usar GLOBAL para configuración global)',

    -- Módulos / Servicios
    auto_welcome_dm                 BOOLEAN NOT NULL DEFAULT TRUE,
    auto_role_sync                  BOOLEAN NOT NULL DEFAULT TRUE,
    auto_nick_sync                  BOOLEAN NOT NULL DEFAULT TRUE,
    auto_voice_channels             BOOLEAN NOT NULL DEFAULT TRUE,
    auto_move_players               BOOLEAN NOT NULL DEFAULT TRUE,
    match_announcements             BOOLEAN NOT NULL DEFAULT TRUE,
    server_status_announcements     BOOLEAN NOT NULL DEFAULT TRUE,
    live_match_panel                BOOLEAN NOT NULL DEFAULT TRUE,
    webhook_validation              BOOLEAN NOT NULL DEFAULT FALSE,
    admin_call_notifications        BOOLEAN NOT NULL DEFAULT TRUE,
    hub_bans_notifications          BOOLEAN NOT NULL DEFAULT TRUE,
    music_bot                       BOOLEAN NOT NULL DEFAULT TRUE,
    recharge_wallet_system          BOOLEAN NOT NULL DEFAULT TRUE,
    mixton_ready_panel              BOOLEAN NOT NULL DEFAULT TRUE,

    -- Comandos Slash de Discord
    command_verificar               BOOLEAN NOT NULL DEFAULT TRUE,
    command_desvincular             BOOLEAN NOT NULL DEFAULT TRUE,
    command_leaderboard             BOOLEAN NOT NULL DEFAULT TRUE,
    command_stats                   BOOLEAN NOT NULL DEFAULT TRUE,
    command_info                    BOOLEAN NOT NULL DEFAULT TRUE,
    command_sincronizar             BOOLEAN NOT NULL DEFAULT TRUE,
    command_partidas                BOOLEAN NOT NULL DEFAULT TRUE,
    command_crear_sala              BOOLEAN NOT NULL DEFAULT TRUE,
    command_mixton_listo            BOOLEAN NOT NULL DEFAULT TRUE,
    command_mixton_no_listo         BOOLEAN NOT NULL DEFAULT TRUE,
    command_cs2_play                BOOLEAN NOT NULL DEFAULT TRUE,
    command_sancionar               BOOLEAN NOT NULL DEFAULT TRUE,
    command_desancionar             BOOLEAN NOT NULL DEFAULT TRUE,
    command_say                     BOOLEAN NOT NULL DEFAULT TRUE,
    command_recargar                BOOLEAN NOT NULL DEFAULT TRUE,
    command_setup_recargas          BOOLEAN NOT NULL DEFAULT TRUE,
    command_setup_soporte           BOOLEAN NOT NULL DEFAULT TRUE,
    command_llamar_admin            BOOLEAN NOT NULL DEFAULT TRUE,

    updated_at                      DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3) ON UPDATE CURRENT_TIMESTAMP(3),

    PRIMARY KEY (guild_id),
    CONSTRAINT fk_feature_flags_guild FOREIGN KEY (guild_id) REFERENCES guilds (guild_id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- Colas de juego configuradas por servidor (HubQueue / queues array)
CREATE TABLE IF NOT EXISTS guild_queues (
    id                              BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
    guild_id                        VARCHAR(32) NOT NULL,
    queue_code                      VARCHAR(64) NOT NULL COMMENT 'Campo id dentro de HubQueue',
    queue_id                        VARCHAR(64) NOT NULL COMMENT 'ID de la cola en FACEIT o interna',
    name                            VARCHAR(128) NOT NULL,
    discord_matches_channel_id      VARCHAR(32) NULL,
    discord_waiting_room_channel_id VARCHAR(32) NULL,
    discord_mixton_channel_id       VARCHAR(32) NULL,
    discord_mixton_chat_channel_id  VARCHAR(32) NULL,
    discord_match_access_role_id    VARCHAR(32) NULL,
    hub_role_id                     VARCHAR(32) NULL,
    discord_hub_role_id             VARCHAR(32) NULL,
    sort_order                      INT UNSIGNED NOT NULL DEFAULT 0,
    created_at                      DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    updated_at                      DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3) ON UPDATE CURRENT_TIMESTAMP(3),

    PRIMARY KEY (id),
    UNIQUE KEY uk_guild_queue_code (guild_id, queue_code),
    KEY idx_guild_queues_guild (guild_id, sort_order),
    CONSTRAINT fk_queues_guild FOREIGN KEY (guild_id) REFERENCES guilds (guild_id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- Plantillas de mensajes (faceit_config/messages)
CREATE TABLE IF NOT EXISTS message_templates (
    guild_id                        VARCHAR(32) NOT NULL DEFAULT 'GLOBAL',
    welcome_title                    VARCHAR(255) NOT NULL,
    welcome_description             TEXT NOT NULL,
    welcome_color                   VARCHAR(16) NOT NULL DEFAULT '#FF5500',
    support_title                   VARCHAR(255) NOT NULL,
    support_description             TEXT NOT NULL,
    support_color                   VARCHAR(16) NOT NULL DEFAULT '#FF0000',
    extra_templates                 JSON NULL,
    updated_at                      DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3) ON UPDATE CURRENT_TIMESTAMP(3),

    PRIMARY KEY (guild_id),
    CONSTRAINT fk_templates_guild FOREIGN KEY (guild_id) REFERENCES guilds (guild_id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- Paneles interactivos publicados en Discord (panel_recargas, panel_soporte, status_panel)
CREATE TABLE IF NOT EXISTS discord_panels (
    id                              BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
    guild_id                        VARCHAR(32) NOT NULL,
    panel_type                      ENUM('live_status', 'mixton_ready', 'recharge', 'support', 'verification') NOT NULL,
    channel_id                      VARCHAR(32) NOT NULL,
    message_id                      VARCHAR(32) NOT NULL,
    extra_data                      JSON NULL,
    updated_at                      DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3) ON UPDATE CURRENT_TIMESTAMP(3),

    PRIMARY KEY (id),
    UNIQUE KEY uk_guild_panel_type (guild_id, panel_type),
    CONSTRAINT fk_panels_guild FOREIGN KEY (guild_id) REFERENCES guilds (guild_id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;


-- ============================================================================
-- 2. USUARIOS Y SANCIONES GLOBALES
--    La vinculación y desvinculación es 100% global (independiente del servidor).
--    Un usuario se identifica únicamente por su discord_id en todos los servidores.
-- ============================================================================

CREATE TABLE IF NOT EXISTS users (
    discord_id                      VARCHAR(32) NOT NULL COMMENT 'ID de usuario de Discord (Snowflake)',
    discord_username                VARCHAR(64) NULL,
    discord_display_name            VARCHAR(128) NULL,
    discord_avatar                  VARCHAR(512) NULL,

    -- Perfil de FACEIT
    faceit_player_id                VARCHAR(64) NULL,
    faceit_nickname                 VARCHAR(64) NULL,
    faceit_nickname_lower           VARCHAR(64) GENERATED ALWAYS AS (LOWER(faceit_nickname)) STORED,
    faceit_elo                      INT UNSIGNED NOT NULL DEFAULT 1000,
    faceit_level                    TINYINT UNSIGNED NOT NULL DEFAULT 1,
    faceit_level_season             TINYINT UNSIGNED NULL,
    faceit_avatar_url               VARCHAR(512) NULL,
    faceit_country                  VARCHAR(8) NULL,

    -- Perfil de Steam
    steam_id_64                     VARCHAR(32) NULL,
    steam_nickname                  VARCHAR(128) NULL,
    steam_avatar_url                VARCHAR(512) NULL,

    -- Contacto, permisos y atributos competitivos globales
    phone_number                    VARCHAR(20) NULL COMMENT 'Celular peruano de 9 dígitos para Yape',
    rol                             ENUM('user', 'mod', 'admin', 'global_admin') NOT NULL DEFAULT 'user',
    status                          ENUM('Verificado', 'Pendiente', 'Baneado') NOT NULL DEFAULT 'Verificado',
    role_type                       ENUM('level', 'special') NULL,
    role_value                      VARCHAR(64) NULL,
    club_role                       VARCHAR(64) NULL,
    mixton_tier_override            ENUM('S', 'A', 'B', 'C') NULL,

    -- Billetera (almacenada en céntimos enteros para exactitud financiera)
    wallet_balance_cents            BIGINT NOT NULL DEFAULT 0,
    wallet_balance                  DECIMAL(12, 2) GENERATED ALWAYS AS (wallet_balance_cents / 100.0) STORED,
    last_balance_update             BIGINT NULL COMMENT 'Epoch ms de última actualización de saldo',
    last_balance_reason             VARCHAR(255) NULL,
    last_withdrawal_at              BIGINT NULL COMMENT 'Epoch ms del último retiro (para cooldown de 60s)',

    -- Servidor principal o de origen (informativo)
    primary_guild_id                VARCHAR(32) NULL,

    -- Marcas de tiempo
    last_hub_match_at               DATETIME(3) NULL,
    linked_at                       DATETIME(3) NULL,
    created_at                      DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    updated_at                      DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3) ON UPDATE CURRENT_TIMESTAMP(3),

    extra_data                      JSON NULL,

    PRIMARY KEY (discord_id),
    KEY idx_users_faceit_nick_lower (faceit_nickname_lower),
    KEY idx_users_faceit_player_id (faceit_player_id),
    KEY idx_users_steam_id_64 (steam_id_64),
    KEY idx_users_primary_guild (primary_guild_id),
    CONSTRAINT chk_users_wallet_non_negative CHECK (wallet_balance_cents >= 0)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- Verificaciones pendientes (faceit_pending_verifications)
CREATE TABLE IF NOT EXISTS pending_verifications (
    id                              BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
    discord_id                      VARCHAR(32) NOT NULL,
    guild_id                        VARCHAR(32) NULL,
    faceit_nickname                 VARCHAR(64) NULL,
    steam_id_64                     VARCHAR(32) NULL,
    phone_number                    VARCHAR(20) NULL,
    verification_token              VARCHAR(128) NULL,
    status                          ENUM('pending', 'completed', 'expired') NOT NULL DEFAULT 'pending',
    expires_at                      DATETIME(3) NULL,
    created_at                      DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),

    PRIMARY KEY (id),
    KEY idx_pending_verif_discord (discord_id, status),
    CONSTRAINT fk_pending_verif_user FOREIGN KEY (discord_id) REFERENCES users (discord_id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- Historial de sanciones y baneos del Hub (hub_bans_cache + /sancionar + /desancionar)
CREATE TABLE IF NOT EXISTS sanctions (
    id                              BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
    guild_id                        VARCHAR(32) NULL,
    discord_id                      VARCHAR(32) NOT NULL,
    faceit_player_id                VARCHAR(64) NULL,
    staff_discord_id                VARCHAR(32) NULL COMMENT 'Administrador que aplicó la sanción',
    reason                          VARCHAR(512) NOT NULL DEFAULT 'Comportamiento antideportivo / Violación de reglas del Hub',
    duration_label                  VARCHAR(64) NOT NULL DEFAULT 'Permanente' COMMENT 'Ej: 12h, 24h, 3d, 7d, Permanente',
    source                          ENUM('discord_command', 'faceit_hub', 'web_admin') NOT NULL DEFAULT 'discord_command',
    is_active                       BOOLEAN NOT NULL DEFAULT TRUE,
    starts_at                       DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    expires_at                      DATETIME(3) NULL,
    revoked_by_discord_id           VARCHAR(32) NULL,
    revoked_at                      DATETIME(3) NULL,
    created_at                      DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),

    PRIMARY KEY (id),
    KEY idx_sanctions_user_active (discord_id, is_active),
    KEY idx_sanctions_guild_created (guild_id, created_at DESC),
    CONSTRAINT fk_sanctions_user FOREIGN KEY (discord_id) REFERENCES users (discord_id) ON DELETE CASCADE,
    CONSTRAINT fk_sanctions_guild FOREIGN KEY (guild_id) REFERENCES guilds (guild_id) ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;


-- ============================================================================
-- 3. CATÁLOGO DE MAPAS CS2 Y SALAS MIXTÓN (LOBBY, DRAFT, VETO, CHAT)
--    Equivale a: faceit_config/mixton_lobby_{guild}, mixton_chat_{guild},
--                crates/lobby/src/state.rs
-- ============================================================================

CREATE TABLE IF NOT EXISTS maps_catalog (
    map_id                          VARCHAR(64) NOT NULL COMMENT 'Ej: de_mirage, de_inferno',
    name                            VARCHAR(64) NOT NULL,
    image_url                       VARCHAR(512) NOT NULL,
    icon_url                        VARCHAR(512) NOT NULL,
    default_active                  BOOLEAN NOT NULL DEFAULT TRUE,
    is_official_premier             BOOLEAN NOT NULL DEFAULT FALSE,
    is_custom                       BOOLEAN NOT NULL DEFAULT FALSE,
    sort_order                      INT UNSIGNED NOT NULL DEFAULT 0,
    created_at                      DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),

    PRIMARY KEY (map_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- Configuración del catálogo de mapas por servidor (override de active)
CREATE TABLE IF NOT EXISTS guild_map_catalog (
    guild_id                        VARCHAR(32) NOT NULL,
    map_id                          VARCHAR(64) NOT NULL,
    active                          BOOLEAN NOT NULL DEFAULT TRUE,
    custom_image_url                VARCHAR(512) NULL,
    custom_icon_url                 VARCHAR(512) NULL,
    updated_at                      DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3) ON UPDATE CURRENT_TIMESTAMP(3),

    PRIMARY KEY (guild_id, map_id),
    CONSTRAINT fk_guild_maps_guild FOREIGN KEY (guild_id) REFERENCES guilds (guild_id) ON DELETE CASCADE,
    CONSTRAINT fk_guild_maps_map FOREIGN KEY (map_id) REFERENCES maps_catalog (map_id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- Estado del Lobby Mixtón por servidor
CREATE TABLE IF NOT EXISTS mixton_lobbies (
    guild_id                        VARCHAR(32) NOT NULL,
    lobby_mode                      VARCHAR(32) NOT NULL DEFAULT 'auto',
    games_per_match                 TINYINT UNSIGNED NOT NULL DEFAULT 1,
    rounds_per_game                 TINYINT UNSIGNED NOT NULL DEFAULT 1,
    command_enabled                 BOOLEAN NOT NULL DEFAULT TRUE,

    -- Estado del Draft de capitanes
    draft_started                   BOOLEAN NOT NULL DEFAULT FALSE,
    draft_finished                  BOOLEAN NOT NULL DEFAULT FALSE,
    pick_order                      JSON NULL COMMENT 'Ej: ["team_1", "team_2"]',
    pick_sequence                   JSON NULL COMMENT 'Secuencia snake draft de picks',
    current_pick_index              INT UNSIGNED NOT NULL DEFAULT 0,

    -- Propuestas de apuesta de los capitanes
    captain_a_proposal              JSON NULL,
    captain_b_proposal              JSON NULL,
    captain_a_ready                 BOOLEAN NOT NULL DEFAULT FALSE,
    captain_b_ready                 BOOLEAN NOT NULL DEFAULT FALSE,
    bet_agreed                      BOOLEAN NOT NULL DEFAULT FALSE,
    agreed_bet_amount_cents         BIGINT NULL,
    effective_bet_amount_cents      BIGINT NOT NULL DEFAULT 0,
    auto_countdown_ends_at          BIGINT NULL,

    -- Estado de la Ruleta
    roulette_winner_team_id         VARCHAR(32) NULL,
    roulette_winner_name            VARCHAR(128) NULL,
    roulette_spinning               BOOLEAN NOT NULL DEFAULT FALSE,
    roulette_started_at             BIGINT NULL,
    roulette_slice_offset           DOUBLE NULL,

    -- Torneo activo en el lobby (si aplica)
    tournament_json                 JSON NULL,

    updated_at_ms                   BIGINT NOT NULL DEFAULT 0,
    updated_at                      DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3) ON UPDATE CURRENT_TIMESTAMP(3),

    PRIMARY KEY (guild_id),
    CONSTRAINT fk_mixton_lobby_guild FOREIGN KEY (guild_id) REFERENCES guilds (guild_id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- Equipos dentro del Lobby Mixtón (team_1, team_2)
CREATE TABLE IF NOT EXISTS mixton_lobby_teams (
    guild_id                        VARCHAR(32) NOT NULL,
    team_id                         VARCHAR(32) NOT NULL COMMENT 'team_1 o team_2',
    name                            VARCHAR(64) NOT NULL DEFAULT 'Equipo',
    tag                             VARCHAR(16) NOT NULL DEFAULT 'EQ',
    captain_discord_id              VARCHAR(32) NULL,

    PRIMARY KEY (guild_id, team_id),
    CONSTRAINT fk_lobby_teams_lobby FOREIGN KEY (guild_id) REFERENCES mixton_lobbies (guild_id) ON DELETE CASCADE,
    CONSTRAINT fk_lobby_teams_captain FOREIGN KEY (captain_discord_id) REFERENCES users (discord_id) ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- Jugadores presentes en el Lobby Mixtón (en cola disponible o elegidos en un equipo)
CREATE TABLE IF NOT EXISTS mixton_lobby_players (
    guild_id                        VARCHAR(32) NOT NULL,
    discord_id                      VARCHAR(32) NOT NULL,
    role_in_lobby                   ENUM('available', 'captain', 'team_player') NOT NULL DEFAULT 'available',
    team_id                         VARCHAR(32) NULL COMMENT 'NULL si está en availablePlayers, team_1 o team_2 si está en un equipo',
    pick_order_index                INT UNSIGNED NULL,

    nickname                        VARCHAR(64) NOT NULL,
    faceit_nickname                 VARCHAR(64) NULL,
    steam_id_64                     VARCHAR(32) NULL,
    avatar                          VARCHAR(512) NULL,
    faceit_elo                      INT UNSIGNED NOT NULL DEFAULT 1000,
    faceit_level                    TINYINT UNSIGNED NOT NULL DEFAULT 1,
    ready                           BOOLEAN NOT NULL DEFAULT FALSE,
    tier                            ENUM('S', 'A', 'B', 'C') NOT NULL DEFAULT 'C',
    mixton_tier_override            ENUM('S', 'A', 'B', 'C') NULL,
    joined_at                       DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),

    PRIMARY KEY (guild_id, discord_id),
    KEY idx_lobby_players_team (guild_id, team_id, role_in_lobby),
    KEY idx_lobby_players_tier (guild_id, tier, faceit_elo DESC),
    CONSTRAINT fk_lobby_players_lobby FOREIGN KEY (guild_id) REFERENCES mixton_lobbies (guild_id) ON DELETE CASCADE,
    CONSTRAINT fk_lobby_players_user FOREIGN KEY (discord_id) REFERENCES users (discord_id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- Estado del Veto de Mapas del Lobby (MapVetoState)
CREATE TABLE IF NOT EXISTS mixton_map_vetoes (
    guild_id                        VARCHAR(32) NOT NULL,
    active                          BOOLEAN NOT NULL DEFAULT TRUE,
    completed                       BOOLEAN NOT NULL DEFAULT FALSE,
    team_id_a                       VARCHAR(32) NOT NULL DEFAULT 'team_1',
    team_id_b                       VARCHAR(32) NOT NULL DEFAULT 'team_2',
    games_per_match                 TINYINT UNSIGNED NOT NULL DEFAULT 1,
    veto_mode                       VARCHAR(16) NOT NULL DEFAULT 'bo1',
    action_type                     VARCHAR(16) NOT NULL DEFAULT 'ban',
    current_step_index              INT UNSIGNED NOT NULL DEFAULT 0,
    current_ban_index               INT UNSIGNED NOT NULL DEFAULT 0,
    turn                            VARCHAR(32) NOT NULL DEFAULT 'team_1',
    current_step                    VARCHAR(16) NOT NULL DEFAULT 'ban',
    final_chosen_map                VARCHAR(64) NULL,
    bet_amount_cents                BIGINT NOT NULL DEFAULT 0,
    queue_id                        VARCHAR(64) NOT NULL DEFAULT 'auto',
    match_mode                      VARCHAR(32) NOT NULL DEFAULT 'steam',
    started_at_ms                   BIGINT NULL,
    turn_ends_at_ms                 BIGINT NULL,
    auto_launch_ends_at_ms          BIGINT NULL,

    -- Listas de pasos y mapas del veto
    pool_json                       JSON NOT NULL,
    steps_json                      JSON NOT NULL,
    banned_maps_json                JSON NOT NULL,
    picked_maps_json                JSON NOT NULL,
    ban_history_json                JSON NOT NULL,
    chosen_map_json                 JSON NULL,
    chosen_maps_json                JSON NOT NULL,
    ban_sequence_json               JSON NOT NULL,
    active_maps_json                JSON NOT NULL,

    updated_at                      DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3) ON UPDATE CURRENT_TIMESTAMP(3),

    PRIMARY KEY (guild_id),
    CONSTRAINT fk_map_veto_lobby FOREIGN KEY (guild_id) REFERENCES mixton_lobbies (guild_id) ON DELETE CASCADE,
    CONSTRAINT fk_map_veto_final_map FOREIGN KEY (final_chosen_map) REFERENCES maps_catalog (map_id) ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- Historial de Chat del Lobby Mixtón (ChatMessage)
CREATE TABLE IF NOT EXISTS mixton_chat_messages (
    id                              VARCHAR(64) NOT NULL,
    guild_id                        VARCHAR(32) NOT NULL,
    discord_id                      VARCHAR(32) NOT NULL,
    author                          VARCHAR(64) NOT NULL,
    avatar                          VARCHAR(512) NULL,
    is_admin                        BOOLEAN NOT NULL DEFAULT FALSE,
    source                          ENUM('web', 'discord') NOT NULL DEFAULT 'web',
    content                         TEXT NOT NULL,
    image_url                       VARCHAR(512) NULL,
    timestamp_ms                    BIGINT NOT NULL,
    created_at                      DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),

    PRIMARY KEY (id),
    KEY idx_mixton_chat_guild_ts (guild_id, timestamp_ms DESC),
    CONSTRAINT fk_chat_guild FOREIGN KEY (guild_id) REFERENCES guilds (guild_id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;


-- ============================================================================
-- 4. PARTIDAS (ACTIVAS E HISTORIAL), ESTADÍSTICAS Y SERVIDORES CS2
--    Equivale a: faceit_active_matches, faceit_matches_history,
--                faceit_match_stats, cs2_pending_matches, Cs2ServerManager
-- ============================================================================

CREATE TABLE IF NOT EXISTS matches (
    match_id                        VARCHAR(128) NOT NULL COMMENT 'ID de FACEIT, mixton_{ts} o manual_{ts}',
    guild_id                        VARCHAR(32) NULL,
    queue_id                        VARCHAR(64) NULL,
    launch_mode                     ENUM('faceit', 'steam', 'manual') NOT NULL DEFAULT 'faceit',
    lifecycle_state                 ENUM('pending', 'active', 'finished', 'cancelled') NOT NULL DEFAULT 'active'
                                    COMMENT 'active = faceit_active_matches; finished = faceit_matches_history',
    status                          VARCHAR(32) NOT NULL DEFAULT 'ONGOING' COMMENT 'Configuring, READY, ONGOING, FINISHED, CANCELLED',

    -- Servidor CS2 e infraestructura
    map_name                        VARCHAR(64) NULL,
    location                        VARCHAR(64) NOT NULL DEFAULT 'Lima, Perú',
    server_ip                       VARCHAR(64) NULL COMMENT 'Ej: 96.0.23.220:27015',
    assigned_port                   SMALLINT UNSIGNED NULL COMMENT 'Puerto del pool 27015..27019',
    tournament_match_id             VARCHAR(64) NULL,

    -- Apuestas y pozo (en céntimos y decimal)
    bet_amount_cents                BIGINT NOT NULL DEFAULT 0,
    bet_amount                      DECIMAL(12, 2) GENERATED ALWAYS AS (bet_amount_cents / 100.0) STORED,
    total_pot_cents                 BIGINT NOT NULL DEFAULT 0,
    total_pot                       DECIMAL(12, 2) GENERATED ALWAYS AS (total_pot_cents / 100.0) STORED,

    -- Equipos y resultado
    faction1_name                   VARCHAR(128) NOT NULL DEFAULT 'Equipo 1',
    faction1_logo                   VARCHAR(512) NULL,
    faction1_score                  INT UNSIGNED NOT NULL DEFAULT 0,
    faction1_vc_id                  VARCHAR(32) NULL COMMENT 'Canal de voz temporal en Discord para Equipo 1',

    faction2_name                   VARCHAR(128) NOT NULL DEFAULT 'Equipo 2',
    faction2_logo                   VARCHAR(512) NULL,
    faction2_score                  INT UNSIGNED NOT NULL DEFAULT 0,
    faction2_vc_id                  VARCHAR(32) NULL COMMENT 'Canal de voz temporal en Discord para Equipo 2',

    winner                           VARCHAR(32) NULL COMMENT 'team1, team2, faction1, faction2',

    -- Enlaces y recursos
    demo_url                        VARCHAR(512) NULL,
    faceit_url                      VARCHAR(512) NULL,

    -- Rosters en JSON para lectura directa rápida compatible con plan_payout y el frontend
    faction1_roster                 JSON NULL,
    faction2_roster                 JSON NULL,
    matchzy_config_json             JSON NULL COMMENT 'JSON generado por GeneratedMatchZy::build',
    extra_data                      JSON NULL,

    started_at_ms                   BIGINT NULL,
    published_at_ms                 BIGINT NULL,
    created_at                      DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    updated_at                      DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3) ON UPDATE CURRENT_TIMESTAMP(3),
    finished_at                     DATETIME(3) NULL,

    PRIMARY KEY (match_id),
    KEY idx_matches_state_guild_created (lifecycle_state, guild_id, created_at DESC),
    KEY idx_matches_guild_created (guild_id, created_at DESC),
    KEY idx_matches_created_desc (created_at DESC),
    KEY idx_matches_active_port (lifecycle_state, assigned_port),
    CONSTRAINT fk_matches_guild FOREIGN KEY (guild_id) REFERENCES guilds (guild_id) ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- Jugadores participantes por partida (relación normalizada de faction1_roster y faction2_roster)
CREATE TABLE IF NOT EXISTS match_players (
    id                              BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
    match_id                        VARCHAR(128) NOT NULL,
    discord_id                      VARCHAR(32) NULL,
    faction                         ENUM('faction1', 'faction2') NOT NULL,
    is_captain                      BOOLEAN NOT NULL DEFAULT FALSE,
    faceit_player_id                VARCHAR(64) NULL,
    nickname                        VARCHAR(64) NOT NULL,
    steam_id_64                     VARCHAR(32) NULL,
    avatar_url                      VARCHAR(512) NULL,
    faceit_level                    TINYINT UNSIGNED NULL,
    faceit_elo                      INT UNSIGNED NULL,

    PRIMARY KEY (id),
    UNIQUE KEY uk_match_player_discord (match_id, discord_id),
    KEY idx_match_players_user (discord_id),
    KEY idx_match_players_steam (steam_id_64),
    CONSTRAINT fk_match_players_match FOREIGN KEY (match_id) REFERENCES matches (match_id) ON DELETE CASCADE,
    CONSTRAINT fk_match_players_user FOREIGN KEY (discord_id) REFERENCES users (discord_id) ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- Estadísticas detalladas de la partida (faceit_match_stats)
CREATE TABLE IF NOT EXISTS match_stats (
    match_id                        VARCHAR(128) NOT NULL,
    rounds_count                    TINYINT UNSIGNED NOT NULL DEFAULT 1,
    raw_stats_json                  JSON NOT NULL COMMENT 'Documento completo compatible con GET /api/lobby/matches-history/{id}',
    created_at                      DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    updated_at                      DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3) ON UPDATE CURRENT_TIMESTAMP(3),

    PRIMARY KEY (match_id),
    CONSTRAINT fk_match_stats_match FOREIGN KEY (match_id) REFERENCES matches (match_id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- Estadísticas individuales de cada jugador por mapa en una partida
CREATE TABLE IF NOT EXISTS match_player_stats (
    id                              BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
    match_id                        VARCHAR(128) NOT NULL,
    map_number                      TINYINT UNSIGNED NOT NULL DEFAULT 1,
    map_name                        VARCHAR(64) NULL,
    discord_id                      VARCHAR(32) NULL,
    faceit_player_id                VARCHAR(64) NULL,
    steam_id_64                     VARCHAR(32) NULL,
    nickname                        VARCHAR(64) NOT NULL,
    faction                         ENUM('faction1', 'faction2') NOT NULL,

    kills                           SMALLINT UNSIGNED NOT NULL DEFAULT 0,
    deaths                          SMALLINT UNSIGNED NOT NULL DEFAULT 0,
    assists                         SMALLINT UNSIGNED NOT NULL DEFAULT 0,
    kd_ratio                        DECIMAL(5, 2) NOT NULL DEFAULT 0.00,
    kr_ratio                        DECIMAL(5, 2) NOT NULL DEFAULT 0.00,
    adr                             DECIMAL(6, 2) NOT NULL DEFAULT 0.00,
    headshots                       SMALLINT UNSIGNED NOT NULL DEFAULT 0,
    headshots_pct                   DECIMAL(5, 2) NOT NULL DEFAULT 0.00,
    mvps                            SMALLINT UNSIGNED NOT NULL DEFAULT 0,
    triple_kills                    SMALLINT UNSIGNED NOT NULL DEFAULT 0,
    quadro_kills                    SMALLINT UNSIGNED NOT NULL DEFAULT 0,
    penta_kills                     SMALLINT UNSIGNED NOT NULL DEFAULT 0,

    PRIMARY KEY (id),
    KEY idx_mps_match_map (match_id, map_number),
    KEY idx_mps_user (discord_id),
    CONSTRAINT fk_mps_match FOREIGN KEY (match_id) REFERENCES matches (match_id) ON DELETE CASCADE,
    CONSTRAINT fk_mps_user FOREIGN KEY (discord_id) REFERENCES users (discord_id) ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;


-- ============================================================================
-- 5. ECONOMÍA YAPE: TRANSACCIONES, OPERACIONES, RETIROS, PREMIOS Y AUDITORÍA
--    Equivale a: yape_transacciones, yape_operations, yape_withdrawals,
--                match_payouts, yape_audit_logs
-- ============================================================================

-- Pagos detectados por el monitor de notificaciones Yape (yape_transacciones)
CREATE TABLE IF NOT EXISTS yape_transactions (
    operation_number                VARCHAR(64) NOT NULL COMMENT 'Número de operación Yape (id del documento)',
    tipo                            ENUM('ENTRADA', 'SALIDA') NOT NULL DEFAULT 'ENTRADA',
    monto_cents                     BIGINT NOT NULL,
    monto                           DECIMAL(12, 2) GENERATED ALWAYS AS (monto_cents / 100.0) STORED,
    sender                          VARCHAR(128) NULL COMMENT 'Nombre del remitente en la notificación Yape',
    status                          VARCHAR(32) NOT NULL DEFAULT 'DETECTED',
    fecha_hora                      VARCHAR(64) NULL,
    fecha_registro                  VARCHAR(64) NULL,
    detected_at                     DATETIME(3) NULL,
    created_at_ms                   BIGINT NULL,
    raw_payload                     JSON NULL,
    created_at                      DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    -- Las escribe el monitor Yape (database.py / server.js).
    telefono                        VARCHAR(32) NULL,
    mensaje                         VARCHAR(255) NULL,
    comprobante_url                 VARCHAR(255) NULL,

    PRIMARY KEY (operation_number),
    KEY idx_yape_tx_detected (detected_at DESC),
    KEY idx_yape_tx_created_ms (created_at_ms DESC)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- Operaciones de recarga Yape reclamadas/acreditadas por usuarios (yape_operations)
CREATE TABLE IF NOT EXISTS yape_operations (
    operation_number                VARCHAR(64) NOT NULL COMMENT 'Número de operación Yape único (evita doble acreditación)',
    discord_id                      VARCHAR(32) NOT NULL,
    guild_id                        VARCHAR(32) NULL,
    amount_cents                    BIGINT NOT NULL,
    amount                          DECIMAL(12, 2) GENERATED ALWAYS AS (amount_cents / 100.0) STORED,
    credited                        BOOLEAN NOT NULL DEFAULT FALSE,
    status                          ENUM('pending', 'credited', 'rejected') NOT NULL DEFAULT 'pending',
    received_at                     VARCHAR(64) NULL,
    manual_approval_by              VARCHAR(32) NULL,
    manual_approval_at              BIGINT NULL,
    credited_at                     DATETIME(3) NULL,
    created_at                      DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    updated_at                      DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3) ON UPDATE CURRENT_TIMESTAMP(3),

    PRIMARY KEY (operation_number),
    KEY idx_yape_ops_user (discord_id, created_at DESC),
    KEY idx_yape_ops_created (created_at DESC),
    CONSTRAINT fk_yape_ops_user FOREIGN KEY (discord_id) REFERENCES users (discord_id) ON DELETE RESTRICT,
    CONSTRAINT fk_yape_ops_guild FOREIGN KEY (guild_id) REFERENCES guilds (guild_id) ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- Solicitudes de retiro de saldo (yape_withdrawals)
CREATE TABLE IF NOT EXISTS yape_withdrawals (
    id                              VARCHAR(96) NOT NULL COMMENT 'Formato: wd_{now_ms}_{seq}_{discord_id}',
    discord_id                      VARCHAR(32) NOT NULL,
    type                            VARCHAR(32) NOT NULL DEFAULT 'withdrawal',
    -- 'review': no lo paga el worker; un administrador lo pasa a 'pending' o lo rechaza.
    status                          ENUM('pending', 'processing', 'completed', 'rejected', 'cancelled', 'review') NOT NULL DEFAULT 'pending',
    amount_cents                    BIGINT NOT NULL,
    amount                          DECIMAL(12, 2) GENERATED ALWAYS AS (amount_cents / 100.0) STORED,
    phone_number                    CHAR(9) NOT NULL COMMENT 'Celular peruano de 9 dígitos que empieza con 9',
    balance_before_cents            BIGINT NOT NULL,
    balance_before                  DECIMAL(12, 2) GENERATED ALWAYS AS (balance_before_cents / 100.0) STORED,
    balance_after_cents             BIGINT NOT NULL,
    balance_after                   DECIMAL(12, 2) GENERATED ALWAYS AS (balance_after_cents / 100.0) STORED,
    source                          VARCHAR(32) NOT NULL DEFAULT 'web',
    processed_by                    VARCHAR(32) NULL,
    processed_at                    DATETIME(3) NULL,
    admin_note                      VARCHAR(255) NULL,
    created_at                      DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    updated_at                      DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3) ON UPDATE CURRENT_TIMESTAMP(3),
    -- Las escribe el worker de retiros Yape al completar el envío.
    comprobante_url                 VARCHAR(255) NULL,
    operation_number                VARCHAR(64) NULL,

    PRIMARY KEY (id),
    KEY idx_withdrawals_status_created (status, created_at ASC),
    KEY idx_withdrawals_user_created (discord_id, created_at DESC),
    CONSTRAINT fk_withdrawals_user FOREIGN KEY (discord_id) REFERENCES users (discord_id) ON DELETE RESTRICT,
    CONSTRAINT chk_withdrawals_min_amount CHECK (amount_cents >= 100),
    CONSTRAINT chk_withdrawals_phone CHECK (phone_number REGEXP '^9[0-9]{8}$')
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- Registro de pagos de premios de partidas (match_payouts - garantiza idempotencia)
CREATE TABLE IF NOT EXISTS match_payouts (
    match_id                        VARCHAR(128) NOT NULL,
    status                          ENUM('paid', 'reverted') NOT NULL DEFAULT 'paid',
    bet_cents                       BIGINT NOT NULL,
    total_pot_cents                 BIGINT NOT NULL,
    source                          VARCHAR(32) NOT NULL DEFAULT 'matchzy',
    created_at                      DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),

    PRIMARY KEY (match_id),
    CONSTRAINT fk_payouts_match FOREIGN KEY (match_id) REFERENCES matches (match_id) ON DELETE RESTRICT
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- Ganadores individuales de cada pozo de partida (WinnerShare dentro de match_payouts)
CREATE TABLE IF NOT EXISTS match_payout_winners (
    id                              BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
    match_id                        VARCHAR(128) NOT NULL,
    discord_id                      VARCHAR(32) NOT NULL,
    amount_cents                    BIGINT NOT NULL,
    amount                          DECIMAL(12, 2) GENERATED ALWAYS AS (amount_cents / 100.0) STORED,

    PRIMARY KEY (id),
    UNIQUE KEY uk_payout_winner (match_id, discord_id),
    KEY idx_payout_winners_user (discord_id),
    CONSTRAINT fk_payout_winners_payout FOREIGN KEY (match_id) REFERENCES match_payouts (match_id) ON DELETE CASCADE,
    CONSTRAINT fk_payout_winners_user FOREIGN KEY (discord_id) REFERENCES users (discord_id) ON DELETE RESTRICT
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- Bitácora de auditoría financiera (yape_audit_logs)
CREATE TABLE IF NOT EXISTS yape_audit_logs (
    id                              VARCHAR(128) NOT NULL COMMENT 'Ej: credited_{ms}_{op}, withdrawal_{id}, payout_{match}_{user}',
    event                           VARCHAR(64) NOT NULL COMMENT 'credited, withdrawal_requested, match_payout, manual_approve',
    action                          VARCHAR(64) NULL,
    source                          VARCHAR(32) NOT NULL DEFAULT 'web' COMMENT 'web, discord, matchzy, admin',
    discord_id                      VARCHAR(32) NOT NULL,
    guild_id                        VARCHAR(32) NULL,
    amount_cents                    BIGINT NOT NULL,
    amount                          DECIMAL(12, 2) GENERATED ALWAYS AS (amount_cents / 100.0) STORED,
    operation_number                VARCHAR(128) NULL,
    reason                          VARCHAR(255) NULL,
    timestamp_ms                    BIGINT NULL,
    created_at                      DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),

    PRIMARY KEY (id),
    KEY idx_audit_user_created (discord_id, created_at DESC),
    KEY idx_audit_event_created (event, created_at DESC),
    KEY idx_audit_timestamp_ms (timestamp_ms DESC),
    CONSTRAINT fk_audit_user FOREIGN KEY (discord_id) REFERENCES users (discord_id) ON DELETE RESTRICT
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;


-- ============================================================================
-- 6. TIENDA, CATÁLOGO DE PRODUCTOS Y ÓRDENES DE COMPRA
--    Equivale a: crates/web/src/routes/products.json y colección orders
-- ============================================================================

CREATE TABLE IF NOT EXISTS products (
    id                              VARCHAR(64) NOT NULL COMMENT 'Ej: prod_torneo_diario, prod_pack_10',
    name                            VARCHAR(128) NOT NULL,
    category                        VARCHAR(64) NOT NULL,
    price_cents                     BIGINT NOT NULL,
    price                           DECIMAL(10, 2) GENERATED ALWAYS AS (price_cents / 100.0) STORED,
    currency                        CHAR(3) NOT NULL DEFAULT 'PEN',
    badge                           VARCHAR(64) NULL,
    description                     TEXT NOT NULL,
    image_url                       VARCHAR(512) NOT NULL,
    features_json                   JSON NOT NULL,
    active                          BOOLEAN NOT NULL DEFAULT TRUE,
    sort_order                      INT UNSIGNED NOT NULL DEFAULT 0,
    created_at                      DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    updated_at                      DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3) ON UPDATE CURRENT_TIMESTAMP(3),

    PRIMARY KEY (id),
    KEY idx_products_active_order (active, sort_order)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE IF NOT EXISTS orders (
    order_id                        VARCHAR(64) NOT NULL COMMENT 'Ej: ORD-2026-0001',
    customer_discord_id             VARCHAR(32) NULL,
    customer_name                   VARCHAR(128) NOT NULL,
    customer_email                  VARCHAR(128) NOT NULL,
    customer_phone                  VARCHAR(32) NULL,
    total_amount_cents              BIGINT NOT NULL,
    total_amount                    DECIMAL(12, 2) GENERATED ALWAYS AS (total_amount_cents / 100.0) STORED,
    currency                        CHAR(3) NOT NULL DEFAULT 'PEN',
    payment_method                  VARCHAR(32) NOT NULL DEFAULT 'YAPE',
    charge_id                       VARCHAR(128) NULL,
    reference_code                  VARCHAR(128) NULL,
    status                          ENUM('PENDIENTE', 'PAGADO', 'COMPLETADA', 'ENTREGADO', 'CANCELADO') NOT NULL DEFAULT 'COMPLETADA',
    admin_note                      TEXT NULL,
    created_at_iso                  VARCHAR(40) NULL,
    updated_at_iso                  VARCHAR(40) NULL,
    created_at                      DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    updated_at                      DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3) ON UPDATE CURRENT_TIMESTAMP(3),

    PRIMARY KEY (order_id),
    KEY idx_orders_status_created (status, created_at DESC),
    KEY idx_orders_customer (customer_discord_id, created_at DESC),
    CONSTRAINT fk_orders_user FOREIGN KEY (customer_discord_id) REFERENCES users (discord_id) ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE IF NOT EXISTS order_items (
    id                              BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
    order_id                        VARCHAR(64) NOT NULL,
    product_id                      VARCHAR(64) NULL,
    product_name                    VARCHAR(128) NOT NULL,
    unit_price_cents                BIGINT NOT NULL,
    unit_price                      DECIMAL(10, 2) GENERATED ALWAYS AS (unit_price_cents / 100.0) STORED,
    quantity                        INT UNSIGNED NOT NULL DEFAULT 1,
    item_total_cents                BIGINT NOT NULL,
    item_total                      DECIMAL(12, 2) GENERATED ALWAYS AS (item_total_cents / 100.0) STORED,

    PRIMARY KEY (id),
    KEY idx_order_items_order (order_id),
    CONSTRAINT fk_order_items_order FOREIGN KEY (order_id) REFERENCES orders (order_id) ON DELETE CASCADE,
    CONSTRAINT fk_order_items_product FOREIGN KEY (product_id) REFERENCES products (id) ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;


-- ============================================================================
-- 7. LIBRO DE RECLAMACIONES (INDECOPI PERÚ)
--    Equivale a: colección claims ({claimCode})
-- ============================================================================

CREATE TABLE IF NOT EXISTS claims (
    claim_code                      VARCHAR(32) NOT NULL COMMENT 'Formato: LR-<año>-<6 dígitos>',

    -- Datos del consumidor
    consumer_name                   VARCHAR(128) NOT NULL,
    consumer_doc_type               VARCHAR(16) NOT NULL COMMENT 'DNI, CE, RUC, PASAPORTE',
    consumer_doc_number             VARCHAR(32) NOT NULL,
    consumer_email                  VARCHAR(128) NOT NULL,
    consumer_phone                  VARCHAR(32) NULL,
    consumer_address                VARCHAR(255) NULL,
    consumer_is_minor               BOOLEAN NOT NULL DEFAULT FALSE,
    parent_name                     VARCHAR(128) NULL,
    parent_doc_number               VARCHAR(32) NULL,

    -- Identificación del bien contratado
    goods_type                      ENUM('PRODUCTO', 'SERVICIO') NOT NULL DEFAULT 'SERVICIO',
    goods_amount                    DECIMAL(12, 2) NOT NULL DEFAULT 0.00,
    goods_currency                  CHAR(3) NOT NULL DEFAULT 'PEN',
    goods_description               TEXT NOT NULL,

    -- Detalle de la reclamación y pedido del consumidor
    claim_type                      ENUM('RECLAMO', 'QUEJA') NOT NULL DEFAULT 'RECLAMO',
    claim_detail                    TEXT NOT NULL,
    claim_request                   TEXT NOT NULL,

    -- Datos del proveedor en el momento del reclamo
    merchant_company_name           VARCHAR(128) NOT NULL DEFAULT 'MachuPeek',
    merchant_address                VARCHAR(255) NOT NULL DEFAULT 'Online',
    merchant_email                  VARCHAR(128) NOT NULL DEFAULT 'machupeek@gmail.com',
    merchant_phone                  VARCHAR(32) NOT NULL DEFAULT '+51 949 165 677',
    legal_notice                    TEXT NULL,

    -- Estado y respuesta administrativa
    status                          ENUM('PENDIENTE', 'EN_PROCESO', 'ATENDIDO', 'RECHAZADO') NOT NULL DEFAULT 'PENDIENTE',
    response_deadline_days          SMALLINT UNSIGNED NOT NULL DEFAULT 15,
    response_text                   TEXT NULL,
    responded_by                    VARCHAR(64) NULL,

    submitted_at                    VARCHAR(40) NULL,
    created_at_iso                  VARCHAR(40) NULL,
    responded_at_iso                VARCHAR(40) NULL,
    created_at                      DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    updated_at                      DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3) ON UPDATE CURRENT_TIMESTAMP(3),
    responded_at                    DATETIME(3) NULL,
    extra_data                      JSON NULL,

    PRIMARY KEY (claim_code),
    KEY idx_claims_status_created (status, created_at DESC),
    KEY idx_claims_created_desc (created_at DESC)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;


-- ============================================================================
-- 8. LLAMADAS DE SOPORTE, LOGS DEL SISTEMA Y EVENTOS (HALLOWEEN)
--    Equivale a: faceit_support_calls, faceit_bot_logs, halloween_*
-- ============================================================================

CREATE TABLE IF NOT EXISTS support_calls (
    id                              VARCHAR(64) NOT NULL COMMENT 'ID del llamado de soporte (supportId)',
    guild_id                        VARCHAR(32) NULL,
    match_id                        VARCHAR(128) NULL,
    match_url                       VARCHAR(512) NULL,
    discord_id                      VARCHAR(32) NOT NULL,
    faceit_player_id                VARCHAR(64) NULL,
    faceit_nickname                 VARCHAR(64) NULL,
    description                     TEXT NOT NULL,
    status                          ENUM('pendiente', 'atendida', 'cerrada') NOT NULL DEFAULT 'pendiente',
    claimed_by                      VARCHAR(128) NULL COMMENT 'Ej: WebAdmin (nombre)',
    claimed_at_ms                   BIGINT NULL,
    created_at                      DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    updated_at                      DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3) ON UPDATE CURRENT_TIMESTAMP(3),

    PRIMARY KEY (id),
    KEY idx_support_calls_guild_created (guild_id, created_at DESC),
    KEY idx_support_calls_created (created_at DESC),
    CONSTRAINT fk_support_calls_guild FOREIGN KEY (guild_id) REFERENCES guilds (guild_id) ON DELETE SET NULL,
    CONSTRAINT fk_support_calls_user FOREIGN KEY (discord_id) REFERENCES users (discord_id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE IF NOT EXISTS bot_logs (
    id                              BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
    log_id                          VARCHAR(64) NOT NULL COMMENT 'Ej: log_1793613600000',
    guild_id                        VARCHAR(32) NULL,
    level                           ENUM('info', 'warn', 'error', 'success') NOT NULL DEFAULT 'info',
    source                          VARCHAR(64) NOT NULL DEFAULT 'system' COMMENT 'bot, system, web, matchzy',
    message                         TEXT NOT NULL,
    extra_data                      JSON NULL,
    created_at                      DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),

    PRIMARY KEY (id),
    UNIQUE KEY uk_bot_logs_log_id (log_id),
    KEY idx_bot_logs_guild_created (guild_id, created_at DESC),
    KEY idx_bot_logs_created (created_at DESC),
    CONSTRAINT fk_bot_logs_guild FOREIGN KEY (guild_id) REFERENCES guilds (guild_id) ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE IF NOT EXISTS halloween_events (
    id                              TINYINT UNSIGNED NOT NULL DEFAULT 1,
    active                          BOOLEAN NOT NULL DEFAULT FALSE,
    state_json                      JSON NULL,
    updated_at                      DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3) ON UPDATE CURRENT_TIMESTAMP(3),

    PRIMARY KEY (id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;


-- ============================================================================
-- 9. VISTAS DE COMPATIBILIDAD DIRECTA CON EL CÓDIGO LEGACY Y RUST
-- ============================================================================

-- Vista unificada que expone el usuario global (crates/data/src/users.rs)
CREATE OR REPLACE VIEW v_full_users AS
SELECT
    u.discord_id AS discordId,
    u.discord_username,
    u.discord_display_name,
    u.faceit_player_id,
    u.faceit_nickname,
    u.faceit_nickname_lower,
    u.faceit_elo,
    u.faceit_level,
    u.faceit_level_season,
    u.steam_id_64,
    u.steam_nickname,
    u.phone_number AS phoneNumber,
    u.rol,
    u.status,
    u.wallet_balance,
    u.wallet_balance_cents,
    u.last_balance_update,
    u.last_balance_reason,
    u.last_withdrawal_at,
    u.primary_guild_id AS guild_id,
    u.primary_guild_id AS servidor,
    u.primary_guild_id AS mixton_guild_id,
    u.club_role AS clubRole,
    u.mixton_tier_override,
    u.linked_at AS linkedAt,
    u.last_hub_match_at,
    u.created_at AS createdAt,
    u.updated_at AS updatedAt
FROM users u;

-- Vista de partidas activas (faceit_active_matches)
CREATE OR REPLACE VIEW v_faceit_active_matches AS
SELECT *
FROM matches
WHERE lifecycle_state = 'active';

-- Vista de historial de partidas (faceit_matches_history)
CREATE OR REPLACE VIEW v_faceit_matches_history AS
SELECT *
FROM matches
WHERE lifecycle_state = 'finished';


-- ============================================================================
-- 10. PROCEDIMIENTOS ALMACENADOS TRANSACCIONALES (ACID)
--     Replican las transacciones de Firestore en yape.rs, withdraw.rs y payout.rs
-- ============================================================================

DELIMITER $$

-- 10.1 Acreditar operación Yape verificada (crates/data/src/yape.rs::credit_yape_operation)
DROP PROCEDURE IF EXISTS sp_credit_yape_operation$$
CREATE PROCEDURE sp_credit_yape_operation(
    IN p_operation_number VARCHAR(64),
    IN p_discord_id VARCHAR(32),
    IN p_guild_id VARCHAR(32),
    IN p_expected_amount_cents BIGINT,
    IN p_phone VARCHAR(20),
    IN p_source VARCHAR(32),
    OUT p_status VARCHAR(32),
    OUT p_new_balance_cents BIGINT
)
proc_label: BEGIN
    DECLARE v_tx_tipo VARCHAR(16);
    DECLARE v_tx_monto_cents BIGINT;
    DECLARE v_tx_fecha VARCHAR(64);
    DECLARE v_op_discord_id VARCHAR(32);
    DECLARE v_op_credited BOOLEAN;
    DECLARE v_now_ms BIGINT;

    DECLARE EXIT HANDLER FOR SQLEXCEPTION
    BEGIN
        ROLLBACK;
        RESIGNAL;
    END;

    START TRANSACTION;

    -- 1. Verificar que el depósito exista en el monitor (yape_transactions)
    SELECT tipo, monto_cents, COALESCE(fecha_hora, fecha_registro)
      INTO v_tx_tipo, v_tx_monto_cents, v_tx_fecha
      FROM yape_transactions
     WHERE operation_number = p_operation_number
     FOR UPDATE;

    IF v_tx_tipo IS NULL THEN
        ROLLBACK;
        SET p_status = 'NOT_YET_SEEN';
        SET p_new_balance_cents = 0;
        LEAVE proc_label;
    END IF;

    IF UPPER(v_tx_tipo) <> 'ENTRADA' THEN
        ROLLBACK;
        SET p_status = 'INVALID_TYPE';
        SET p_new_balance_cents = 0;
        LEAVE proc_label;
    END IF;

    IF v_tx_monto_cents <> p_expected_amount_cents THEN
        ROLLBACK;
        SET p_status = 'AMOUNT_MISMATCH';
        SET p_new_balance_cents = v_tx_monto_cents;
        LEAVE proc_label;
    END IF;

    -- 2. Verificar que la operación no haya sido acreditada ya
    SELECT discord_id, credited
      INTO v_op_discord_id, v_op_credited
      FROM yape_operations
     WHERE operation_number = p_operation_number
     FOR UPDATE;

    IF v_op_discord_id IS NOT NULL THEN
        IF v_op_credited = TRUE THEN
            ROLLBACK;
            IF v_op_discord_id = p_discord_id THEN
                SET p_status = 'ALREADY_CREDITED_OWN';
            ELSE
                SET p_status = 'ALREADY_CREDITED_OTHER';
            END IF;
            SET p_new_balance_cents = 0;
            LEAVE proc_label;
        END IF;

        IF v_op_discord_id <> p_discord_id THEN
            ROLLBACK;
            SET p_status = 'RESERVED_BY_OTHER';
            SET p_new_balance_cents = 0;
            LEAVE proc_label;
        END IF;
    END IF;

    SET v_now_ms = ROUND(UNIX_TIMESTAMP(CURRENT_TIMESTAMP(3)) * 1000);

    -- 3. Asegurar que el usuario exista e incrementar su saldo atómicamente
    INSERT INTO users (
        discord_id, phone_number, wallet_balance_cents,
        last_balance_update, last_balance_reason, primary_guild_id
    ) VALUES (
        p_discord_id, p_phone, v_tx_monto_cents,
        v_now_ms, CONCAT('Recarga Yape verificada (operación: ', p_operation_number, ')'), p_guild_id
    )
    ON DUPLICATE KEY UPDATE
        wallet_balance_cents = wallet_balance_cents + v_tx_monto_cents,
        phone_number = COALESCE(NULLIF(p_phone, ''), phone_number),
        last_balance_update = v_now_ms,
        last_balance_reason = CONCAT('Recarga Yape verificada (operación: ', p_operation_number, ')');

    -- 4. Marcar la operación como acreditada
    INSERT INTO yape_operations (
        operation_number, discord_id, guild_id, amount_cents,
        credited, status, received_at, credited_at
    ) VALUES (
        p_operation_number, p_discord_id, p_guild_id, v_tx_monto_cents,
        TRUE, 'credited', v_tx_fecha, CURRENT_TIMESTAMP(3)
    )
    ON DUPLICATE KEY UPDATE
        credited = TRUE,
        status = 'credited',
        received_at = v_tx_fecha,
        credited_at = CURRENT_TIMESTAMP(3);

    -- 5. Registrar auditoría
    INSERT INTO yape_audit_logs (
        id, event, source, discord_id, guild_id,
        amount_cents, operation_number, timestamp_ms
    ) VALUES (
        CONCAT('credited_', v_now_ms, '_', p_operation_number),
        'credited', p_source, p_discord_id, p_guild_id,
        v_tx_monto_cents, p_operation_number, v_now_ms
    );

    SELECT wallet_balance_cents INTO p_new_balance_cents
      FROM users
     WHERE discord_id = p_discord_id;

    COMMIT;
    SET p_status = 'CREDITED';
END$$


-- 10.2 Solicitar retiro de saldo Yape (crates/data/src/withdraw.rs::request_withdrawal)
DROP PROCEDURE IF EXISTS sp_request_withdrawal$$
CREATE PROCEDURE sp_request_withdrawal(
    IN p_withdrawal_id VARCHAR(96),
    IN p_discord_id VARCHAR(32),
    IN p_amount_cents BIGINT,
    IN p_phone CHAR(9),
    IN p_source VARCHAR(32),
    OUT p_status VARCHAR(32),
    OUT p_new_balance_cents BIGINT
)
proc_label: BEGIN
    DECLARE v_balance_cents BIGINT;
    DECLARE v_last_wd_ms BIGINT;
    DECLARE v_now_ms BIGINT;

    DECLARE EXIT HANDLER FOR SQLEXCEPTION
    BEGIN
        ROLLBACK;
        RESIGNAL;
    END;

    IF p_amount_cents < 100 THEN
        SET p_status = 'BELOW_MIN_AMOUNT';
        SET p_new_balance_cents = 0;
        LEAVE proc_label;
    END IF;

    START TRANSACTION;

    -- Bloquear fila del usuario para evitar doble retiro concurrente
    SELECT wallet_balance_cents, last_withdrawal_at
      INTO v_balance_cents, v_last_wd_ms
      FROM users
     WHERE discord_id = p_discord_id
     FOR UPDATE;

    IF v_balance_cents IS NULL THEN
        ROLLBACK;
        SET p_status = 'USER_NOT_FOUND';
        SET p_new_balance_cents = 0;
        LEAVE proc_label;
    END IF;

    IF p_amount_cents > v_balance_cents THEN
        ROLLBACK;
        SET p_status = 'INSUFFICIENT_FUNDS';
        SET p_new_balance_cents = v_balance_cents;
        LEAVE proc_label;
    END IF;

    SET v_now_ms = ROUND(UNIX_TIMESTAMP(CURRENT_TIMESTAMP(3)) * 1000);

    -- Cooldown de 60,000 ms (60 segundos)
    IF v_last_wd_ms IS NOT NULL AND (v_now_ms - v_last_wd_ms) BETWEEN 0 AND 59999 THEN
        ROLLBACK;
        SET p_status = 'COOLDOWN';
        SET p_new_balance_cents = v_balance_cents;
        LEAVE proc_label;
    END IF;

    SET p_new_balance_cents = v_balance_cents - p_amount_cents;

    UPDATE users
       SET wallet_balance_cents = p_new_balance_cents,
           last_withdrawal_at = v_now_ms,
           last_balance_update = v_now_ms,
           last_balance_reason = CONCAT('Solicitud de retiro Yape (', p_withdrawal_id, ')')
     WHERE discord_id = p_discord_id;

    INSERT INTO yape_withdrawals (
        id, discord_id, type, status, amount_cents,
        phone_number, balance_before_cents, balance_after_cents, source
    ) VALUES (
        p_withdrawal_id, p_discord_id, 'withdrawal', 'pending', p_amount_cents,
        p_phone, v_balance_cents, p_new_balance_cents, p_source
    );

    INSERT INTO yape_audit_logs (
        id, event, source, discord_id, amount_cents, operation_number, timestamp_ms
    ) VALUES (
        CONCAT('withdrawal_', p_withdrawal_id),
        'withdrawal_requested', p_source, p_discord_id, p_amount_cents, p_withdrawal_id, v_now_ms
    );

    COMMIT;
    SET p_status = 'REQUESTED';
END$$


-- 10.3 Pagar pozo de partida a ganadores con idempotencia (crates/data/src/payout.rs::pay_match_winners)
DROP PROCEDURE IF EXISTS sp_pay_match_winners$$
CREATE PROCEDURE sp_pay_match_winners(
    IN p_match_id VARCHAR(128),
    IN p_guild_id VARCHAR(32),
    IN p_winning_faction VARCHAR(32),
    IN p_pot_cents BIGINT,
    IN p_source VARCHAR(32),
    OUT p_status VARCHAR(32),
    OUT p_per_winner_cents BIGINT
)
proc_label: BEGIN
    DECLARE v_existing_match VARCHAR(128);
    DECLARE v_winner_count INT DEFAULT 0;
    DECLARE v_now_ms BIGINT;

    DECLARE EXIT HANDLER FOR SQLEXCEPTION
    BEGIN
        ROLLBACK;
        RESIGNAL;
    END;

    IF p_pot_cents <= 0 THEN
        SET p_status = 'INVALID_POT';
        SET p_per_winner_cents = 0;
        LEAVE proc_label;
    END IF;

    START TRANSACTION;

    -- 1. Idempotencia: verificar si esta partida ya fue pagada
    SELECT match_id INTO v_existing_match
      FROM match_payouts
     WHERE match_id = p_match_id
     FOR UPDATE;

    IF v_existing_match IS NOT NULL THEN
        ROLLBACK;
        SET p_status = 'ALREADY_PAID';
        SET p_per_winner_cents = 0;
        LEAVE proc_label;
    END IF;

    -- 2. Contar ganadores vinculados de la facción ganadora
    SELECT COUNT(DISTINCT discord_id) INTO v_winner_count
      FROM match_players
     WHERE match_id = p_match_id
       AND faction = p_winning_faction
       AND discord_id IS NOT NULL;

    IF v_winner_count = 0 THEN
        ROLLBACK;
        SET p_status = 'EMPTY_WINNERS';
        SET p_per_winner_cents = 0;
        LEAVE proc_label;
    END IF;

    -- División entera en céntimos (igual que pot_cents / n en payout.rs)
    SET p_per_winner_cents = p_pot_cents DIV v_winner_count;
    IF p_per_winner_cents <= 0 THEN
        ROLLBACK;
        SET p_status = 'SHARE_TOO_SMALL';
        SET p_per_winner_cents = 0;
        LEAVE proc_label;
    END IF;

    SET v_now_ms = ROUND(UNIX_TIMESTAMP(CURRENT_TIMESTAMP(3)) * 1000);

    -- 3. Registrar cabecera del pago (match_payouts)
    INSERT INTO match_payouts (
        match_id, guild_id, winning_faction, pot_cents,
        per_winner_cents, winner_count, source, paid_at_ms
    ) VALUES (
        p_match_id, p_guild_id, p_winning_faction, p_pot_cents,
        p_per_winner_cents, v_winner_count, p_source, v_now_ms
    );

    -- 4. Registrar cada ganador en match_payout_winners
    INSERT INTO match_payout_winners (match_id, discord_id, share_cents)
    SELECT DISTINCT p_match_id, discord_id, p_per_winner_cents
      FROM match_players
     WHERE match_id = p_match_id
       AND faction = p_winning_faction
       AND discord_id IS NOT NULL;

    -- 5. Acreditar saldo atómicamente a todos los ganadores
    UPDATE users u
      JOIN match_payout_winners mpw
        ON mpw.discord_id = u.discord_id
       AND mpw.match_id = p_match_id
       SET u.wallet_balance_cents = u.wallet_balance_cents + p_per_winner_cents,
           u.last_balance_update = v_now_ms,
           u.last_balance_reason = CONCAT('Premio partida ', p_match_id, ' (', p_winning_faction, ')');

    -- 6. Marcar la partida como pagada
    UPDATE matches
       SET paid = TRUE,
           winner = p_winning_faction
     WHERE match_id = p_match_id;

    -- 7. Auditoría financiera
    INSERT INTO yape_audit_logs (
        id, event, source, guild_id, match_id, amount_cents, timestamp_ms
    ) VALUES (
        CONCAT('payout_', p_match_id),
        'match_payout', p_source, p_guild_id, p_match_id, p_pot_cents, v_now_ms
    );

    COMMIT;
    SET p_status = 'PAID';
END$$

DELIMITER ;


-- ============================================================================
-- 11. DATOS SEMILLA POR DEFECTO (SEED DATA)
--     Garantizan que el sistema arranque con los mismos valores que hoy
-- ============================================================================

-- 11.1 Servidor GLOBAL y Servidor Principal de MachuPeek
INSERT INTO guilds (
    guild_id, name, is_default,
    match_timeout_minutes, games_per_match, rounds_per_game
) VALUES
    ('GLOBAL', 'Configuración Global MachuPeek', FALSE, 120, 1, 1),
    ('1274598531759345674', 'MachuPeek CS2 Perú', TRUE, 120, 1, 1),
    ('1472731390675255410', 'MachuPeek Hub Oficial', FALSE, 120, 1, 1)
ON DUPLICATE KEY UPDATE
    name = VALUES(name);

-- 11.2 Feature Flags por defecto (idénticos a LEGACY_DEFAULTS en global_settings.rs)
INSERT INTO guild_feature_flags (guild_id)
VALUES ('GLOBAL'), ('1274598531759345674')
ON DUPLICATE KEY UPDATE updated_at = CURRENT_TIMESTAMP(3);

-- 11.3 Plantillas de mensajes por defecto (default_message_templates.json)
INSERT INTO message_templates (
    guild_id,
    welcome_title,
    welcome_description,
    welcome_color,
    support_title,
    support_description,
    support_color
) VALUES (
    'GLOBAL',
    '¡Bienvenido al Servidor! 🎉',
    '¡Hola **{username}**! Gracias por unirte a nuestra comunidad.\n\nAquí tienes una **guía rápida** para unirte y verificar tu cuenta de **Faceit**:\n\n📌 **PASO 1: Únete al Club**\nHaz clic en el siguiente enlace y únete a nuestro club oficial en la plataforma:\n🔗 **[Hacer clic aquí para Unirse al Club]({clubUrl})**\n\n📌 **PASO 2: Obtén tu Nickname de Faceit**\nAsegúrate de copiar tu nombre de usuario exacto tal y como aparece en tu perfil de Faceit.\n\n📌 **PASO 3: Vincula tu cuenta en Discord**\nVe al canal de Discord {linkChannelMention} y escribe el comando:\n➡️ `/verificar_fc`\n\nEl bot te pedirá tu nickname de Faceit. Escríbelo, presiona enviar y ¡listo! Tus roles de nivel y del Club se actualizarán automáticamente.\n\n¡Nos vemos en el servidor y buena suerte en tus partidas! 🎮',
    '#FF5500',
    '🚨 Soporte y Llamado de Admin',
    '¿Tienes algún problema en tu partida activa de Faceit?\n\nPresiona el botón de abajo para solicitar la asistencia de un administrador del staff.\n\n⚠️ **Requisitos y Notas Importantes**:\n- Debes tener tu cuenta de Faceit vinculada con el bot (usa `/verificar_fc` si aún no lo has hecho).\n- Solo funcionará si te encuentras actualmente en una **partida activa** de Faceit en este servidor/club.\n- Al presionar el botón, se enviará una notificación con el enlace a tu partida al equipo de soporte.',
    '#FF0000'
) ON DUPLICATE KEY UPDATE
    welcome_title = VALUES(welcome_title);

-- 11.4 Catálogo oficial de 14 mapas de CS2 (crates/web/src/routes/admin/matches_users.rs)
INSERT INTO maps_catalog (map_id, name, image_url, icon_url, default_active, is_official_premier, sort_order) VALUES
    ('de_mirage',   'Mirage',   'https://raw.githubusercontent.com/ggMartinez/CS2-Maps-Images/main/maps/de_mirage.png',   'https://raw.githubusercontent.com/MurkyYT/cs2-map-icons/main/images/de_mirage.png',   TRUE,  TRUE,  1),
    ('de_inferno',  'Inferno',  'https://raw.githubusercontent.com/ggMartinez/CS2-Maps-Images/main/maps/de_inferno.png',  'https://raw.githubusercontent.com/MurkyYT/cs2-map-icons/main/images/de_inferno.png',  TRUE,  TRUE,  2),
    ('de_dust2',    'Dust II',  'https://raw.githubusercontent.com/ggMartinez/CS2-Maps-Images/main/maps/de_dust2.png',    'https://raw.githubusercontent.com/MurkyYT/cs2-map-icons/main/images/de_dust2.png',    TRUE,  TRUE,  3),
    ('de_nuke',     'Nuke',     'https://raw.githubusercontent.com/ggMartinez/CS2-Maps-Images/main/maps/de_nuke.png',     'https://raw.githubusercontent.com/MurkyYT/cs2-map-icons/main/images/de_nuke.png',     TRUE,  TRUE,  4),
    ('de_ancient',  'Ancient',  'https://raw.githubusercontent.com/ggMartinez/CS2-Maps-Images/main/maps/de_ancient.png',  'https://raw.githubusercontent.com/MurkyYT/cs2-map-icons/main/images/de_ancient.png',  TRUE,  TRUE,  5),
    ('de_anubis',   'Anubis',   'https://raw.githubusercontent.com/ggMartinez/CS2-Maps-Images/main/maps/de_anubis.png',   'https://raw.githubusercontent.com/MurkyYT/cs2-map-icons/main/images/de_anubis.png',   TRUE,  TRUE,  6),
    ('de_vertigo',  'Vertigo',  'https://raw.githubusercontent.com/ggMartinez/CS2-Maps-Images/main/maps/de_vertigo.png',  'https://raw.githubusercontent.com/MurkyYT/cs2-map-icons/main/images/de_vertigo.png',  TRUE,  TRUE,  7),
    ('de_train',    'Train',    'https://raw.githubusercontent.com/ggMartinez/CS2-Maps-Images/main/maps/de_train.png',    'https://raw.githubusercontent.com/MurkyYT/cs2-map-icons/main/images/de_train.png',    FALSE, FALSE, 8),
    ('de_overpass', 'Overpass', 'https://raw.githubusercontent.com/ggMartinez/CS2-Maps-Images/main/maps/de_overpass.png', 'https://raw.githubusercontent.com/MurkyYT/cs2-map-icons/main/images/de_overpass.png', FALSE, FALSE, 9),
    ('de_cache',    'Cache',    'https://raw.githubusercontent.com/ggMartinez/CS2-Maps-Images/main/maps/de_cache.png',    'https://raw.githubusercontent.com/MurkyYT/cs2-map-icons/main/images/de_cache.png',    FALSE, FALSE, 10),
    ('cs_office',   'Office',   'https://raw.githubusercontent.com/ggMartinez/CS2-Maps-Images/main/maps/cs_office.png',   'https://raw.githubusercontent.com/MurkyYT/cs2-map-icons/main/images/cs_office.png',   FALSE, FALSE, 11),
    ('cs_italy',    'Italy',    'https://raw.githubusercontent.com/ggMartinez/CS2-Maps-Images/main/maps/cs_italy.png',    'https://raw.githubusercontent.com/MurkyYT/cs2-map-icons/main/images/cs_italy.png',    FALSE, FALSE, 12),
    ('de_thera',    'Thera',    'https://raw.githubusercontent.com/ggMartinez/CS2-Maps-Images/main/maps/de_thera.png',    'https://raw.githubusercontent.com/MurkyYT/cs2-map-icons/main/images/de_thera.png',    FALSE, FALSE, 13),
    ('de_mills',    'Mills',    'https://raw.githubusercontent.com/ggMartinez/CS2-Maps-Images/main/maps/de_mills.png',    'https://raw.githubusercontent.com/MurkyYT/cs2-map-icons/main/images/de_mills.png',    FALSE, FALSE, 14)
ON DUPLICATE KEY UPDATE
    name = VALUES(name),
    default_active = VALUES(default_active),
    is_official_premier = VALUES(is_official_premier);

-- 11.5 Catálogo de productos de la tienda (crates/web/src/routes/products.json)
INSERT INTO products (id, name, category, price_cents, currency, badge, description, image_url, features_json, sort_order) VALUES
    (
        'prod_torneo_diario',
        'Entrada a Torneo Mixtón Diario',
        'Torneos',
        500,
        'PEN',
        'Popular',
        'Pase de acceso individual para 1 jornada completa de torneos Mixtón en servidores oficiales con anticheat FACEIT. Emparejamiento por Tiers competitivos (S, A, B, C) y registro de estadísticas.',
        'https://images.unsplash.com/photo-1542751371-adc38448a05e?auto=format&fit=crop&w=600&q=80',
        JSON_ARRAY('Acceso a partidas clasificatorias del día', 'Servidores dedicados de bajo ping en Lima', 'Anticheat oficial de FACEIT activo', 'Registro de estadísticas K/D y ADR'),
        1
    ),
    (
        'prod_pack_10',
        'Pack Gamer - 10 Créditos de Pozo',
        'Créditos',
        1000,
        'PEN',
        'Básico',
        'Paquete de 10 créditos para participar en cuotas y pozos de premios de partidas competitivas en la Arena Mixtón. Acreditación automática e inmediata a tu billetera.',
        'https://images.unsplash.com/photo-1614680376593-902f749f7ffc?auto=format&fit=crop&w=600&q=80',
        JSON_ARRAY('10 créditos para cuotas de partidas', 'Participación en pozos de torneos', 'Acreditación inmediata tras verificación del depósito Yape', 'Saldo sin fecha de caducidad'),
        2
    ),
    (
        'prod_pack_25',
        'Pack Pro - 25 Créditos + Insignia',
        'Créditos',
        2500,
        'PEN',
        'Recomendado',
        'Paquete de 25 créditos para torneos y partidas. Incluye insignia distintiva dorada en tu perfil del lobby y prioridad en colas de emparejamiento.',
        'https://images.unsplash.com/photo-1579546929518-9e396f3cc809?auto=format&fit=crop&w=600&q=80',
        JSON_ARRAY('25 créditos para cuotas y torneos', 'Insignia PRO dorada en el lobby', 'Prioridad de asignación en salas', 'Soporte preferencial en Discord'),
        3
    ),
    (
        'prod_vip_30',
        'Membresía VIP Club Mensual (30 Días)',
        'Membresías',
        3500,
        'PEN',
        'VIP',
        'Suscripción mensual con acceso total a salas de entrenamiento exclusivas, registro de estadísticas avanzadas de rendimiento, canal VIP en Discord y soporte prioritario 24/7.',
        'https://images.unsplash.com/photo-1511512578047-dfb367046420?auto=format&fit=crop&w=600&q=80',
        JSON_ARRAY('Acceso irrestricto a salas VIP', 'Rol VIP en Discord con color exclusivo', 'Analítica avanzada de partidas y mapas', 'Sorteos mensuales de skins de CS2'),
        4
    ),
    (
        'prod_season_pass',
        'Season Pass CS2 Arena (Pase Oficial)',
        'Temporadas',
        5000,
        'PEN',
        'Temporada',
        'Inscripción oficial a la tabla de clasificación por premios en efectivo al final de la temporada oficial, con premios en Soles (PEN) para el top 10 y medalla honorífica.',
        'https://images.unsplash.com/photo-1538481199705-c710c4e965fc?auto=format&fit=crop&w=600&q=80',
        JSON_ARRAY('Inscripción al Leaderboard de la Temporada', 'Acceso a pozo final de premios en efectivo', 'Medalla conmemorativa en perfil', 'Pase para las finales de temporada'),
        5
    ),
    (
        'prod_coaching_1h',
        'Sesión de Coaching Táctico 1v1 (60 Min)',
        'Entrenamiento',
        4000,
        'PEN',
        'Pro',
        'Clase táctica de 60 minutos con un jugador de nivel 10 FACEIT. Incluye revisión de repeticiones de partidas, posicionamiento, crosshair placement, utilidades y optimización de binds.',
        'https://images.unsplash.com/photo-1550745165-9bc0b252726f?auto=format&fit=crop&w=600&q=80',
        JSON_ARRAY('60 minutos de análisis 1 a 1 por voz y pantalla', 'Revisión profunda de repeticiones de tus partidas de CS2', 'Guía personalizada de utilidades y setups', 'Plan de mejora individual de puntería y juego'),
        6
    )
ON DUPLICATE KEY UPDATE
    name = VALUES(name),
    price_cents = VALUES(price_cents);

-- 11.6 Estado inicial de Lobby Mixtón y Evento Halloween
INSERT INTO mixton_lobbies (guild_id, lobby_mode, games_per_match, rounds_per_game)
VALUES ('GLOBAL', 'auto', 1, 1), ('1274598531759345674', 'auto', 1, 1)
ON DUPLICATE KEY UPDATE updated_at = CURRENT_TIMESTAMP(3);

INSERT INTO mixton_lobby_teams (guild_id, team_id, name, tag) VALUES
    ('GLOBAL', 'team_1', 'Equipo 1', 'EQ1'),
    ('GLOBAL', 'team_2', 'Equipo 2', 'EQ2'),
    ('1274598531759345674', 'team_1', 'Equipo 1', 'EQ1'),
    ('1274598531759345674', 'team_2', 'Equipo 2', 'EQ2')
ON DUPLICATE KEY UPDATE name = VALUES(name);

INSERT INTO halloween_events (id, active, state_json)
VALUES (1, FALSE, JSON_OBJECT())
ON DUPLICATE KEY UPDATE id = id;

-- 11.7 Sincronización automática desde tablas relacionales externas hacia realtime_documents
DROP TRIGGER IF EXISTS trg_yape_tx_after_insert;
DROP TRIGGER IF EXISTS trg_yape_tx_after_update;

DELIMITER $$

CREATE TRIGGER trg_yape_tx_after_insert
AFTER INSERT ON yape_transactions
FOR EACH ROW
BEGIN
    INSERT INTO realtime_documents (collection, doc_id, doc_json)
    VALUES (
        'yape_transacciones',
        NEW.operation_number,
        JSON_OBJECT(
            'numero_operacion', NEW.operation_number,
            'tipo', NEW.tipo,
            'monto', ROUND(NEW.monto_cents / 100.0, 2),
            'remitente', NEW.sender,
            'telefono', NEW.telefono,
            'mensaje', NEW.mensaje,
            'fecha_hora', NEW.fecha_hora,
            'fecha_registro', NEW.fecha_registro,
            'detectedAt', DATE_FORMAT(NEW.created_at, '%Y-%m-%dT%H:%i:%sZ')
        )
    )
    ON DUPLICATE KEY UPDATE doc_json = VALUES(doc_json);
END$$

CREATE TRIGGER trg_yape_tx_after_update
AFTER UPDATE ON yape_transactions
FOR EACH ROW
BEGIN
    INSERT INTO realtime_documents (collection, doc_id, doc_json)
    VALUES (
        'yape_transacciones',
        NEW.operation_number,
        JSON_OBJECT(
            'numero_operacion', NEW.operation_number,
            'tipo', NEW.tipo,
            'monto', ROUND(NEW.monto_cents / 100.0, 2),
            'remitente', NEW.sender,
            'telefono', NEW.telefono,
            'mensaje', NEW.mensaje,
            'fecha_hora', NEW.fecha_hora,
            'fecha_registro', NEW.fecha_registro,
            'detectedAt', DATE_FORMAT(NEW.created_at, '%Y-%m-%dT%H:%i:%sZ')
        )
    )
    ON DUPLICATE KEY UPDATE doc_json = VALUES(doc_json);
END$$

DELIMITER ;

-- 11.8 Semilla inicial en realtime_documents para consultas y streams en tiempo real
INSERT INTO realtime_documents (collection, doc_id, doc_json) VALUES
    (
        'faceit_config',
        'settings',
        JSON_OBJECT(
            'discordGuildId', '1274598531759345674',
            'discordLinkChannelId', '',
            'discordMatchesChannelId', '',
            'discordMixtonChannelId', '',
            'discordRolesChannelId', '',
            'discordStatusChannelId', '',
            'discordWaitingRoomChannelId', '',
            'discordAdminAlertsChannelId', '',
            'discordSupportChannelId', '',
            'discordMusicChannelId', '',
            'discordSanctionsChannelId', '',
            'autoWelcomeDm', TRUE,
            'autoRoleSync', TRUE,
            'autoNickSync', TRUE,
            'autoVoiceChannels', TRUE,
            'autoMovePlayers', TRUE,
            'matchAnnouncements', TRUE,
            'serverStatusAnnouncements', TRUE,
            'liveMatchPanel', TRUE,
            'webhookValidation', FALSE,
            'adminCallNotifications', TRUE,
            'hubBansNotifications', TRUE,
            'musicBot', TRUE,
            'rechargeWalletSystem', TRUE,
            'mixtonReadyPanel', TRUE,
            'command_verificar', TRUE,
            'command_desvincular', TRUE,
            'command_leaderboard', TRUE,
            'command_stats', TRUE,
            'command_info', TRUE,
            'command_sincronizar', TRUE,
            'command_partidas', TRUE,
            'command_crear_sala', TRUE,
            'command_mixton_listo', TRUE,
            'command_mixton_no_listo', TRUE,
            'command_cs2_play', TRUE,
            'command_sancionar', TRUE,
            'command_desancionar', TRUE,
            'command_say', TRUE,
            'command_recargar', TRUE,
            'command_setup_recargas', TRUE,
            'command_setup_soporte', TRUE,
            'command_llamar_admin', TRUE,
            'faceitHubId', '',
            'adminRoleId', '',
            'matchTimeoutMinutes', 120,
            'gamesPerMatch', 1,
            'roundsPerGame', 1,
            'updatedAt', NULL
        )
    ),
    (
        'guild_settings',
        '1274598531759345674',
        JSON_OBJECT(
            'name', 'MachuPeek CS2 Perú',
            'matchTimeoutMinutes', 120,
            'gamesPerMatch', 1,
            'roundsPerGame', 1
        )
    ),
    (
        'guild_settings',
        '1472731390675255410',
        JSON_OBJECT(
            'name', 'MachuPeek Hub Oficial',
            'matchTimeoutMinutes', 120,
            'gamesPerMatch', 1,
            'roundsPerGame', 1
        )
    ),
    (
        'faceit_config',
        'halloween_event',
        JSON_OBJECT('active', FALSE)
    )
ON DUPLICATE KEY UPDATE
    doc_json = VALUES(doc_json);

SET FOREIGN_KEY_CHECKS = 1;
