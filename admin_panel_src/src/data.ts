import { LinkedUser, LiveMatch, SystemLog, BotConfig, FeatureFlag, PlayerStats } from './types';

export const initialUsers: LinkedUser[] = [
  {
    discordId: '1938472918347',
    faceitNickname: 'ToxicPlayer_99',
    faceitPlayerId: 'fct_88a9s7d6f5g',
    status: 'Verificado',
    linkedAt: 'Oct 24, 2023',
  },
  {
    discordId: '5829104857291',
    faceitNickname: 'NoobMaster',
    faceitPlayerId: 'fct_11b2c3d4e5',
    status: 'Pendiente',
    linkedAt: 'Oct 25, 2023',
  },
  {
    discordId: '7382910485729',
    faceitNickname: 'ProGamer_X',
    faceitPlayerId: 'fct_99x8y7z6w5',
    status: 'Verificado',
    linkedAt: 'Oct 21, 2023',
  },
  {
    discordId: '1122334455667',
    faceitNickname: 'ShadowNinja',
    faceitPlayerId: 'fct_aa11bb22cc',
    status: 'Baneado',
    linkedAt: 'Sep 15, 2023',
  },
];

export const initialMatches: LiveMatch[] = [
  {
    id: '#FCT-9821',
    teamAName: 'NAVI',
    teamALogo: 'https://lh3.googleusercontent.com/aida-public/AB6AXuC-T-ArhtmjNY7XWZGrTNyRSbs6dzTSYUG8mTVsB1cc-xMXXgaw-A_Nu4cjf174jExQDOmINOjV8PY9alMDZE_dqpiQul2ijWV0YQWZQgrC7iJduQ81acIXw9v2AxvKG3SnoRkfjp4q0v_ZON39heKHxDAXz_hQpq5k28s8Oi8dUzMWuaL3zg9h8hRr6AJAtU05Lf9kXnTfRbB4VRKm06T022nyuz4R6IQQMigmy42dT0xrL2KLKquDmdA7DbOORzHNvwbd580bEXo',
    teamBName: 'Faze Clan',
    teamBLogo: 'https://lh3.googleusercontent.com/aida-public/AB6AXuAfrmmYKhxVUoG0tA5zPPKQG3K-0Yz2HYfiYCTrA2uEti-a3oGqCGijpFdJznq_-y4kj2cg-P7Yxtgr7W36dYgThhWVSnaBWcEeKGctwx44Jse6K18ajLmteMLI5FOZd8N45N6Adjcdi_ZEFtotiFFWLTS1nZsdRW8srwGxFawN4egUdDuRlThqA5AtD4rohTaR8xPdyfAInqxC21u86FFAQYlO9S9ClnKQxseloZ9e5MZy6F7mO8gKkgVe8cwA3vtb29XAwaZA1ws',
    map: 'de_mirage',
    location: 'Alemania (FRA)',
    startedAtMinutesAgo: 12,
  },
  {
    id: '#FCT-9822',
    teamAName: 'G2 Esports',
    teamBName: 'Vitality',
    map: 'de_inferno',
    location: 'Reino Unido (LON)',
    startedAtMinutesAgo: 45,
  },
  {
    id: '#FCT-9825',
    teamAName: 'Team Liquid',
    teamBName: 'Complexity',
    map: 'de_nuke',
    location: 'EE. UU. Este (CHI)',
    startedAtMinutesAgo: 2,
  },
  {
    id: '#FCT-9810',
    teamAName: 'MOUZ',
    teamBName: 'ENCE',
    map: 'de_ancient',
    location: 'Suecia (STO)',
    startedAtMinutesAgo: 85, // Flagged as high duration (>60 minutes)
  },
];

export interface FeatureDefinition {
  key: string;
  name: string;
  desc: string;
  defaultEnabled: boolean;
  type: 'module' | 'command';
  category: 'voice_music' | 'faceit_matches' | 'community_roles' | 'mixton_queues' | 'moderation_support' | 'wallet_economy';
  channelKey?: keyof BotConfig | string;
  channelLabel?: string;
}

export const FEATURE_DEFINITIONS: FeatureDefinition[] = [
  // 🎵 MÚSICA & VOZ
  {
    key: 'musicBot',
    name: 'Bot & Reproductor de Música',
    desc: 'Panel persistente en canal de música, reproductor de voz con YouTube/yt-dlp, cola y botones de reproducción.',
    defaultEnabled: true,
    type: 'module',
    category: 'voice_music',
    channelKey: 'discordMusicChannelId',
    channelLabel: 'Canal de Música',
  },
  {
    key: 'autoVoiceChannels',
    name: 'Canales de Voz Temporales',
    desc: 'Creación y eliminación automática de canales de voz por equipo para partidas en vivo.',
    defaultEnabled: true,
    type: 'module',
    category: 'voice_music',
    channelKey: 'discordWaitingRoomChannelId',
    channelLabel: 'Sala de Espera',
  },
  {
    key: 'autoMovePlayers',
    name: 'Auto-Mover Jugadores',
    desc: 'Mover automáticamente a los jugadores de la sala de espera al canal de voz de su equipo.',
    defaultEnabled: true,
    type: 'module',
    category: 'voice_music',
    channelKey: 'discordWaitingRoomChannelId',
    channelLabel: 'Sala de Espera',
  },
  {
    key: 'command_cs2_play',
    name: 'Cmd /cs2_play',
    desc: 'Comando slash para reproducir canciones, playlists o búsquedas de YouTube en el canal de voz actual.',
    defaultEnabled: true,
    type: 'command',
    category: 'voice_music',
    channelKey: 'discordMusicChannelId',
    channelLabel: 'Canal de Música',
  },

  // 🎮 FACEIT & PARTIDAS
  {
    key: 'matchAnnouncements',
    name: 'Anuncios de Partidas',
    desc: 'Publicar embeds automáticos con mapas, servidores y equipos cuando comienza una partida en FACEIT.',
    defaultEnabled: true,
    type: 'module',
    category: 'faceit_matches',
    channelKey: 'discordMatchesChannelId',
    channelLabel: 'Canal de Partidas',
  },
  {
    key: 'liveMatchPanel',
    name: 'Panel de Partidas en Vivo',
    desc: 'Panel interactivo anclado que actualiza en tiempo real marcadores, servidores y estado de partidas.',
    defaultEnabled: true,
    type: 'module',
    category: 'faceit_matches',
    channelKey: 'discordMatchesChannelId',
    channelLabel: 'Canal de Partidas',
  },
  {
    key: 'serverStatusAnnouncements',
    name: 'Estado de Servidores LATAM',
    desc: 'Monitoreo continuo y avisos sobre conectividad y emparejamiento de servidores de FACEIT.',
    defaultEnabled: true,
    type: 'module',
    category: 'faceit_matches',
    channelKey: 'discordStatusChannelId',
    channelLabel: 'Canal de Estado',
  },
  {
    key: 'webhookValidation',
    name: 'Validación de Webhooks',
    desc: 'Verificación criptográfica estricta de la firma HMAC de los webhooks entrantes de FACEIT.',
    defaultEnabled: false,
    type: 'module',
    category: 'faceit_matches',
  },
  {
    key: 'command_partidas',
    name: 'Cmd /partidas',
    desc: 'Comando para que los usuarios consulten todas las partidas activas en curso y enlace a la sala.',
    defaultEnabled: true,
    type: 'command',
    category: 'faceit_matches',
    channelKey: 'discordMatchesChannelId',
    channelLabel: 'Canal de Partidas',
  },
  {
    key: 'command_crear_sala',
    name: 'Cmd /crear_sala',
    desc: 'Comando para armar salas 5v5 equilibradas automáticamente por ELO a partir de jugadores en canal de voz.',
    defaultEnabled: true,
    type: 'command',
    category: 'faceit_matches',
  },

  // 👥 PERFILES & ROLES
  {
    key: 'autoWelcomeDm',
    name: 'Mensaje de Bienvenida (DM)',
    desc: 'Envío automático de guía de bienvenida y pasos para unirse al club y verificarse a nuevos miembros.',
    defaultEnabled: true,
    type: 'module',
    category: 'community_roles',
  },
  {
    key: 'autoRoleSync',
    name: 'Sincronización de Roles',
    desc: 'Asignar y actualizar automáticamente los roles de nivel de FACEIT (1-10) y roles del club en Discord.',
    defaultEnabled: true,
    type: 'module',
    category: 'community_roles',
    channelKey: 'discordRolesChannelId',
    channelLabel: 'Canal de Roles',
  },
  {
    key: 'autoNickSync',
    name: 'Sincronización de Apodos',
    desc: 'Sincronizar apodos de Discord con el nickname verificado de FACEIT al vincular o jugar partidas.',
    defaultEnabled: true,
    type: 'module',
    category: 'community_roles',
  },
  {
    key: 'command_verificar',
    name: 'Cmd /verificar_fc',
    desc: 'Permite a los usuarios vincular su perfil de FACEIT con su cuenta de Discord mediante modal emergente.',
    defaultEnabled: true,
    type: 'command',
    category: 'community_roles',
    channelKey: 'discordLinkChannelId',
    channelLabel: 'Canal de Vinculación',
  },
  {
    key: 'command_desvincular',
    name: 'Cmd /desvincular_fc',
    desc: 'Permite desvincular un perfil de FACEIT de Discord y remover los roles de nivel correspondientes.',
    defaultEnabled: true,
    type: 'command',
    category: 'community_roles',
  },
  {
    key: 'command_sincronizar',
    name: 'Cmd /sincronizar_fc',
    desc: 'Comando para forzar una sincronización manual inmediata de roles, ELO y apodo desde FACEIT.',
    defaultEnabled: true,
    type: 'command',
    category: 'community_roles',
  },
  {
    key: 'command_leaderboard',
    name: 'Cmd /leaderboard',
    desc: 'Muestra la tabla de posiciones y ranking por puntos de la temporada actual del Hub de FACEIT.',
    defaultEnabled: true,
    type: 'command',
    category: 'community_roles',
  },
  {
    key: 'command_stats',
    name: 'Cmd /stats',
    desc: 'Consulta estadísticas detalladas de un jugador (ELO, K/D, porcentaje de victorias, nivel y forma).',
    defaultEnabled: true,
    type: 'command',
    category: 'community_roles',
  },
  {
    key: 'command_info',
    name: 'Cmd /info',
    desc: 'Muestra información técnica del bot, servidores monitoreados, latencia y enlaces oficiales.',
    defaultEnabled: true,
    type: 'command',
    category: 'community_roles',
  },

  // ⚔️ MIXTON 5v5 & COLAS
  {
    key: 'mixtonReadyPanel',
    name: 'Panel de Espera Mixton 5v5',
    desc: 'Panel interactivo fijado que muestra la lista de jugadores que han marcado listo para armar partida.',
    defaultEnabled: true,
    type: 'module',
    category: 'mixton_queues',
    channelKey: 'discordMixtonChannelId',
    channelLabel: 'Canal Mixton',
  },
  {
    key: 'command_mixton_listo',
    name: 'Cmd /listo',
    desc: 'Permite a los jugadores marcarse como listos para entrar a la próxima partida comunitaria de Mixton.',
    defaultEnabled: true,
    type: 'command',
    category: 'mixton_queues',
    channelKey: 'discordMixtonChannelId',
    channelLabel: 'Canal Mixton',
  },
  {
    key: 'command_mixton_no_listo',
    name: 'Cmd /nolisto',
    desc: 'Permite a los jugadores retirarse de la lista de espera de la cola comunitaria de Mixton.',
    defaultEnabled: true,
    type: 'command',
    category: 'mixton_queues',
    channelKey: 'discordMixtonChannelId',
    channelLabel: 'Canal Mixton',
  },

  // 🛡️ MODERACIÓN & SOPORTE
  {
    key: 'adminCallNotifications',
    name: 'Llamados a Administrador',
    desc: 'Sistema de notificaciones de tickets de auxilio para solicitar asistencia del staff en partidas.',
    defaultEnabled: true,
    type: 'module',
    category: 'moderation_support',
    channelKey: 'discordAdminAlertsChannelId',
    channelLabel: 'Canal Alertas Admin',
  },
  {
    key: 'hubBansNotifications',
    name: 'Alertas de Sanciones de Hub',
    desc: 'Rastreador en segundo plano de nuevos baneos aplicados en el Hub de FACEIT y aviso en canal oficial.',
    defaultEnabled: true,
    type: 'module',
    category: 'moderation_support',
    channelKey: 'discordSanctionsChannelId',
    channelLabel: 'Canal de Sanciones',
  },
  {
    key: 'command_llamar_admin',
    name: 'Cmd /llamar_admin',
    desc: 'Permite a un jugador en partida activa solicitar la presencia urgente de un administrador o árbitro.',
    defaultEnabled: true,
    type: 'command',
    category: 'moderation_support',
    channelKey: 'discordSupportChannelId',
    channelLabel: 'Canal de Soporte',
  },
  {
    key: 'command_setup_soporte',
    name: 'Cmd /setup_soporte',
    desc: 'Despliega el panel interactivo fijado con el botón de auxilio y llamado a soporte en el canal actual.',
    defaultEnabled: true,
    type: 'command',
    category: 'moderation_support',
    channelKey: 'discordSupportChannelId',
    channelLabel: 'Canal de Soporte',
  },
  {
    key: 'command_sancionar',
    name: 'Cmd /sancionar',
    desc: 'Aplica y notifica una sanción o suspensión temporal a un jugador con registro en base de datos.',
    defaultEnabled: true,
    type: 'command',
    category: 'moderation_support',
    channelKey: 'discordSanctionsChannelId',
    channelLabel: 'Canal de Sanciones',
  },
  {
    key: 'command_desancionar',
    name: 'Cmd /desancionar',
    desc: 'Levanta la sanción o baneo a un usuario previamente sancionado y notifica la liberación.',
    defaultEnabled: true,
    type: 'command',
    category: 'moderation_support',
  },
  {
    key: 'command_say',
    name: 'Cmd /say',
    desc: 'Permite a administradores enviar comunicados oficiales a través del bot en un canal de texto.',
    defaultEnabled: true,
    type: 'command',
    category: 'moderation_support',
  },

  // 💰 BILLETERA & RECARGAS
  {
    key: 'rechargeWalletSystem',
    name: 'Billetera & Recargas Yape',
    desc: 'Módulo de verificación y acreditación automática de depósitos Yape para saldo de torneos y apuestas.',
    defaultEnabled: true,
    type: 'module',
    category: 'wallet_economy',
    channelKey: 'discordRechargeChannelId',
    channelLabel: 'Canal de Recargas',
  },
  {
    key: 'command_recargar',
    name: 'Cmd /recargar',
    desc: 'Abre el formulario para que el usuario ingrese su número de comprobante Yape y acredite su saldo.',
    defaultEnabled: true,
    type: 'command',
    category: 'wallet_economy',
    channelKey: 'discordRechargeChannelId',
    channelLabel: 'Canal de Recargas',
  },
  {
    key: 'command_setup_recargas',
    name: 'Cmd /setup_recargas',
    desc: 'Despliega el panel interactivo fijado de recarga instantánea con Yape en el canal actual.',
    defaultEnabled: true,
    type: 'command',
    category: 'wallet_economy',
    channelKey: 'discordRechargeChannelId',
    channelLabel: 'Canal de Recargas',
  },
];

export function buildFeatureFlags(settings: Record<string, any> = {}): FeatureFlag[] {
  return FEATURE_DEFINITIONS.map(def => ({
    key: def.key,
    name: def.name,
    desc: def.desc,
    type: def.type,
    category: def.category,
    channelKey: def.channelKey,
    channelLabel: def.channelLabel,
    enabled: def.key === 'webhookValidation' 
      ? settings[def.key] === true 
      : settings[def.key] !== undefined 
        ? settings[def.key] !== false 
        : def.defaultEnabled,
  }));
}

export const initialFeatureFlags: FeatureFlag[] = buildFeatureFlags({});

export const initialConfig: BotConfig = {
  discordGuildId: '111222333444555666',
  discordLinkChannelId: '112233445566778899',
  discordMatchesChannelId: '223344556677889900',
  discordMixtonChannelId: '',
  discordMixtonChatChannelId: '',
  discordRolesChannelId: '334455667788990011',
  discordStatusChannelId: '445566778899001122',
  discordWaitingRoomChannelId: '556677889900112233',
  discordAdminAlertsChannelId: '667788990011223344',
  discordSupportChannelId: '778899001122334455',
  discordMusicChannelId: '',
  discordRechargeChannelId: '',
  discordMatchAccessRoleId: '',
  hubRoleId: '',
  discordHubRoleId: '',
  faceitHubId: 'hub-latam-cs2-pro',
  faceitLeaderboardId: '6a05ea92c18e557389267373',
  adminRoleId: '',
  matchTimeoutMinutes: 120,
  gamesPerMatch: 1,
  roundsPerGame: 1,
};

export const initialLogs: SystemLog[] = [];
