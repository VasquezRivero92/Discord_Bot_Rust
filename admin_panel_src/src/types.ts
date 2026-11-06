export interface LinkedUser {
  discordId: string;
  faceitNickname: string;
  faceitPlayerId: string;
  status: 'Verificado' | 'Pendiente' | 'Baneado';
  linkedAt: string;
  roleType?: 'level' | 'special';
  roleValue?: string;
  faceit_level?: number;
  faceit_level_season?: number;
  wallet_balance?: number;
  steam_id_64?: string;
  steam_nickname?: string;
  rol?: string;
  servidor?: string;
  clubRole?: string;
}

export interface ClaimItem {
  id: string;
  claimCode: string;
  consumer: {
    name: string;
    docType: string;
    docNumber: string;
    email: string;
    phone?: string;
    address?: string;
    isMinor?: boolean;
    parent?: { name: string; docNumber: string } | null;
  };
  goods: {
    type: 'PRODUCTO' | 'SERVICIO';
    amount: number;
    currency: string;
    description: string;
  };
  claim: {
    type: 'RECLAMO' | 'QUEJA';
    detail: string;
    request: string;
  };
  status: 'PENDIENTE' | 'EN_PROCESO' | 'ATENDIDO' | 'RECHAZADO';
  createdAt?: any;
  createdAtIso?: string;
  response?: string;
  respondedBy?: string;
  respondedAt?: any;
  respondedAtIso?: string;
}

export interface OrderItem {
  id: string;
  orderId: string;
  customer: {
    name: string;
    email: string;
    phone?: string;
    discordId?: string;
  };
  items: Array<{
    id: string;
    name: string;
    unitPrice: number;
    quantity: number;
    itemTotal: number;
  }>;
  totalAmount: number;
  currency: string;
  paymentMethod: string;
  chargeId?: string;
  referenceCode?: string;
  status: 'PENDIENTE' | 'PAGADO' | 'COMPLETADA' | 'ENTREGADO' | 'CANCELADO';
  adminNote?: string;
  createdAt?: any;
  createdAtIso?: string;
}

export interface YapeTransactionItem {
  id: string;
  operationNumber?: string;
  numero_operacion?: string;
  amount?: number;
  monto?: number;
  sender?: string;
  detectedAt?: string;
  status?: string;
  createdAt?: number;
}

export interface YapeOperationItem {
  id: string;
  operationNumber?: string;
  discordId?: string;
  amount?: number;
  status?: string;
  createdAt?: any;
  manualApprovalBy?: string;
  manualApprovalAt?: number;
}

export interface YapeAuditLogItem {
  id: string;
  action?: string;
  discordId?: string;
  amount?: number;
  timestamp?: number;
  reason?: string;
  source?: string;
}

export interface LiveMatch {
  id: string;
  teamAName: string;
  teamALogo?: string;
  teamBName: string;
  teamBLogo?: string;
  map: string;
  location: string;
  startedAtMinutesAgo: number;
}

export interface SystemLog {
  id: string;
  level: 'info' | 'warn' | 'error' | 'success';
  message: string;
  timestamp: string;
}

export interface HubQueue {
  id: string;
  queueId: string;
  name: string;
  discordMatchesChannelId: string;
  discordWaitingRoomChannelId: string;
  discordMixtonChannelId?: string;
  discordMixtonChatChannelId?: string;
  discordMatchAccessRoleId?: string;
  hubRoleId?: string;
  discordHubRoleId?: string;
}

export interface BotConfig {
  discordGuildId: string;
  discordLinkChannelId: string;
  discordMatchesChannelId: string;
  discordMixtonChannelId?: string;
  discordMixtonChatChannelId?: string;
  discordRolesChannelId: string;
  discordStatusChannelId: string;
  discordWaitingRoomChannelId: string;
  discordAdminAlertsChannelId: string;
  discordSupportChannelId: string;
  discordMusicChannelId?: string;
  discordRechargeChannelId?: string;
  discordSanctionsChannelId?: string;
  discordMatchAccessRoleId?: string;
  hubRoleId?: string;
  discordHubRoleId?: string;
  faceitHubId: string;
  faceitLeaderboardId: string;
  adminRoleId: string;
  matchTimeoutMinutes: number;
  gamesPerMatch?: number;
  roundsPerGame?: number;
  queues?: HubQueue[];
}

export type FeatureCategory = 
  | 'voice_music' 
  | 'faceit_matches' 
  | 'community_roles' 
  | 'mixton_queues' 
  | 'moderation_support' 
  | 'wallet_economy';

export interface FeatureFlag {
  key: string;
  name: string;
  desc: string;
  enabled: boolean;
  type?: 'module' | 'command';
  category?: FeatureCategory;
  channelKey?: string;
  channelLabel?: string;
}

export interface PlayerStats {
  nickname: string;
  country: string;
  countryCode: string;
  level: number;
  elo: number;
  winRate: number;
  kdRatio: number;
  totalMatches: number;
  membership: string;
  recentForm: ('W' | 'L')[];
  recentPoints: number[];
  globalRank: string;
  countryRank: string;
  avatarUrl: string;
}

export interface MapCatalogItem {
  id: string;
  name: string;
  image: string;
  icon: string;
  active: boolean;
  isOfficialPremier?: boolean;
  isCustom?: boolean;
}
