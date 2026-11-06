import React, { useState, useEffect, useMemo } from 'react';
import { ChannelSelect, channelIdsOf, DiscordChannel } from './ChannelSelect';
import {
  LayoutDashboard,
  Users,
  Gamepad2,
  Trophy,
  BarChart3,
  Settings,
  ToggleLeft,
  ToggleRight,
  FileText,
  Search,
  Bell,
  Moon,
  HelpCircle,
  RefreshCw,
  LogOut,
  CheckCircle2,
  XCircle,
  Trash2,
  UserPlus,
  ArrowUpRight,
  Activity,
  Map,
  Clock,
  Lock,
  ArrowUp,
  Plus,
  ChevronLeft,
  ChevronRight,
  Check,
  ShieldAlert,
  Download,
  Eye,
  ExternalLink,
  X,
  ShoppingBag,
  Wallet,
  DollarSign,
  Scale,
  CheckCircle,
  Music,
  Swords,
  Sliders,
  Flame,
  Layers,
  ListFilter,
  Power,
  Radio,
  Calendar,
  Maximize2,
  Grid,
  Target,
  ChevronDown,
  TrendingUp,
  PanelLeftClose,
  PanelLeftOpen,
  PieChart,
  Building2,
  Phone,
  ArrowDown,
  Filter,
  Layers3
} from 'lucide-react';
import {
  LinkedUser,
  LiveMatch,
  SystemLog,
  BotConfig,
  FeatureFlag,
  FeatureCategory,
  PlayerStats,
  ClaimItem,
  OrderItem,
  YapeTransactionItem,
  YapeOperationItem,
  YapeAuditLogItem,
  MapCatalogItem
} from './types';
import {
  initialUsers,
  initialMatches,
  initialFeatureFlags,
  initialConfig,
  initialLogs,
  buildFeatureFlags,
  FEATURE_DEFINITIONS
} from './data';

// Helper para todas las peticiones a la API — siempre envía cookies de sesión
const apiFetch = (url: string, options: RequestInit = {}) => {
  let finalUrl = url;
  const method = (options.method || 'GET').toUpperCase();
  if (method === 'GET') {
    const separator = url.includes('?') ? '&' : '?';
    finalUrl = `${url}${separator}_t=${Date.now()}`;
  }
  return fetch(finalUrl, {
    credentials: 'include',
    ...options,
    headers: {
      'Content-Type': 'application/json',
      ...(options.headers || {})
    }
  });
};

type SteamLoginStatus = {
  connected: boolean;
  waitingForCode: boolean;
  status: string;
  guardType: 'email' | 'two_factor' | null;
  codeAttempts: number;
  lastCodeWrong: boolean;
  retryAfterMs: number;
  message: string;
};

const defaultSteamLoginStatus: SteamLoginStatus = {
  connected: false,
  waitingForCode: false,
  status: 'idle',
  guardType: null,
  codeAttempts: 0,
  lastCodeWrong: false,
  retryAfterMs: 0,
  message: 'Consulta el estado para iniciar Steam Guard.'
};

export default function App() {
  // Authentication State
  const [isLoggedIn, setIsLoggedIn] = useState<boolean>(false);
  const [isAuthChecked, setIsAuthChecked] = useState<boolean>(false);
  const [currentUser, setCurrentUser] = useState<{ username: string; email?: string; avatar: string; role?: string; servidor?: string } | null>(null);
  const [guilds, setGuilds] = useState<any[]>([]);
  const [selectedGuildId, setSelectedGuildId] = useState<string>('');

  useEffect(() => {
    apiFetch('/api/admin/me').then(res => {
      if (res.ok) {
        res.json().then(data => {
          if (data.user) {
            const userProfile = {
              username: data.user.username,
              avatar: data.user.avatar ? `https://cdn.discordapp.com/avatars/${data.user.userId}/${data.user.avatar}.png` : 'https://cdn.discordapp.com/embed/avatars/0.png',
              role: data.user.role,
              servidor: data.user.servidor
            };
            setCurrentUser(userProfile);
            setIsLoggedIn(true);

            // Fetch guilds
            apiFetch('/api/admin/guilds').then(guildsRes => {
              if (guildsRes.ok) {
                guildsRes.json().then(guildsData => {
                  setGuilds(guildsData.guilds || []);
                  const userGuildId = userProfile.role === 'admin' ? userProfile.servidor : (localStorage.getItem('admin_selected_guild') || '');
                  setSelectedGuildId(userGuildId);
                });
              }
            });
          }
        });
      }
    }).finally(() => setIsAuthChecked(true));
  }, []);

  // System Core States
  const [users, setUsers] = useState<LinkedUser[]>([]);

  const [matches, setMatches] = useState<LiveMatch[]>([]);
  const [matchStats, setMatchStats] = useState<{ serverLoad: number; avgDurationStr: string; failedCleanups: number }>({
    serverLoad: 0,
    avgDurationStr: '0m 0s',
    failedCleanups: 0
  });

  const [logs, setLogs] = useState<SystemLog[]>([]);

  const [systemInfo, setSystemInfo] = useState<{ uptimeSeconds: number; hubRegion: string }>({ uptimeSeconds: 0, hubRegion: 'CARGANDO...' });

  const [featureFlags, setFeatureFlags] = useState<FeatureFlag[]>([]);

  const [config, setConfig] = useState<BotConfig>(initialConfig);
  const [discordChannels, setDiscordChannels] = useState<DiscordChannel[]>([]);
  const [availableDiscordRoles, setAvailableDiscordRoles] = useState<{ id: string; name: string; color: string }[]>([]);
  const [leaderboard, setLeaderboard] = useState<any[]>([]);
  const [supportCalls, setSupportCalls] = useState<any[]>([]);
  // Nuevos estados para Módulos de Comercio, Finanzas y Legal
  const [claims, setClaims] = useState<ClaimItem[]>([]);
  const [orders, setOrders] = useState<OrderItem[]>([]);
  const [yapeData, setYapeData] = useState<{
    transactions: YapeTransactionItem[];
    operations: YapeOperationItem[];
    auditLogs: YapeAuditLogItem[];
  }>({ transactions: [], operations: [], auditLogs: [] });

  // Claims Filter & Pagination States
  const [claimsSearch, setClaimsSearch] = useState<string>('');
  const [claimsFilter, setClaimsFilter] = useState<string>('all');
  const [claimsPage, setClaimsPage] = useState<number>(1);
  const [selectedClaimModal, setSelectedClaimModal] = useState<ClaimItem | null>(null);
  const [claimResponseText, setClaimResponseText] = useState<string>('');
  const [claimResponseStatus, setClaimResponseStatus] = useState<'ATENDIDO' | 'EN_PROCESO' | 'RECHAZADO'>('ATENDIDO');
  const [respondingClaim, setRespondingClaim] = useState<boolean>(false);

  // Orders Filter & Pagination States
  const [ordersSearch, setOrdersSearch] = useState<string>('');
  const [ordersFilter, setOrdersFilter] = useState<string>('all');
  const [ordersPage, setOrdersPage] = useState<number>(1);
  const [selectedOrderModal, setSelectedOrderModal] = useState<OrderItem | null>(null);
  const [updatingOrderStatus, setUpdatingOrderStatus] = useState<boolean>(false);

  // Yape / Finances States
  const [yapeActiveSubtab, setYapeActiveSubtab] = useState<'operations' | 'transactions' | 'audit'>('operations');
  const [yapeSearch, setYapeSearch] = useState<string>('');
  const [manualApproveModalOpen, setManualApproveModalOpen] = useState<boolean>(false);
  const [manualApproveOp, setManualApproveOp] = useState<string>('');
  const [manualApproveDiscordId, setManualApproveDiscordId] = useState<string>('');
  const [manualApproveAmount, setManualApproveAmount] = useState<string>('');
  const [manualApproveReason, setManualApproveReason] = useState<string>('');
  const [approvingYape, setApprovingYape] = useState<boolean>(false);

  // Balance Management Modal
  const [balanceModalOpen, setBalanceModalOpen] = useState<boolean>(false);
  const [balanceTargetUser, setBalanceTargetUser] = useState<LinkedUser | null>(null);
  const [balanceActionType, setBalanceActionType] = useState<'add' | 'subtract' | 'set'>('add');
  const [balanceAmount, setBalanceAmount] = useState<string>('');
  const [balanceReason, setBalanceReason] = useState<string>('');
  const [savingBalance, setSavingBalance] = useState<boolean>(false);

  const [messageTemplates, setMessageTemplates] = useState<{
    welcomeTitle: string;
    welcomeDescription: string;
    welcomeColor: string;
    supportTitle: string;
    supportDescription: string;
    supportColor: string;
  }>({
    welcomeTitle: '',
    welcomeDescription: '',
    welcomeColor: '',
    supportTitle: '',
    supportDescription: '',
    supportColor: ''
  });

  // UI States
  
  // Helper Toast notifier
  const showToast = (message: string, type: 'success' | 'error' | 'info' = 'success') => {
    setToast({ message, type });
    setTimeout(() => {
      setToast(null);
    }, 3000);
  };

  const loadData = async () => {
    if (!isLoggedIn) return;
    const safeFetch = async (url: string) => {
      try {
        const res = await apiFetch(url);
        return res.ok ? res : null;
      } catch (e) {
        console.warn(`Failed to fetch ${url}:`, e);
        return null;
      }
    };

    try {
      const guildQuery = selectedGuildId ? `?guildId=${selectedGuildId}` : '';
      const [usersRes, matchesRes, logsRes, settingsRes, leaderboardRes, systemRes, supportRes, messagesRes, historyRes, rolesRes, channelsRes, claimsRes, ordersRes, yapeRes] = await Promise.all([
        safeFetch(`/api/admin/users${guildQuery}`),
        safeFetch(`/api/admin/matches${guildQuery}`),
        safeFetch(`/api/admin/logs${guildQuery}`),
        safeFetch(selectedGuildId ? `/api/admin/guilds/${selectedGuildId}/settings` : '/api/admin/settings'),
        safeFetch(`/api/admin/leaderboard${guildQuery}`),
        safeFetch(`/api/admin/system${guildQuery}`),
        safeFetch(`/api/admin/support-calls${guildQuery}`),
        safeFetch(`/api/admin/messages${guildQuery}`),
        safeFetch(`/api/admin/matches/history${guildQuery}`),
        safeFetch(`/api/admin/discord/roles${guildQuery}`),
        safeFetch(`/api/admin/discord/channels${guildQuery}`),
        safeFetch('/api/admin/claims'),
        safeFetch('/api/admin/orders'),
        safeFetch('/api/admin/yape/transactions')
      ]);

      if (!usersRes && !matchesRes && !logsRes && !settingsRes && !leaderboardRes && !systemRes) {
        showToast('Error al conectar con la API del bot', 'error');
        return;
      }

      if (usersRes) {
        try {
          const uData = (await usersRes.json()).users || [];
          setUsers(uData.map((u: any) => {
            let parsedDate = new Date().toLocaleString();
            if (u.linkedAt && u.linkedAt._seconds) {
              parsedDate = new Date(u.linkedAt._seconds * 1000).toLocaleString('es-ES');
            } else if (typeof u.linkedAt === 'string') {
              parsedDate = u.linkedAt;
            } else if (u.linked_at) {
              parsedDate = u.linked_at;
            }
            
            return {
              ...u,
              discordId: u.discordId || u.discord_id || '',
              faceitNickname: u.faceitNickname || u.faceit_nickname || '',
              faceitPlayerId: u.faceitPlayerId || u.faceit_id || u.faceit_player_id || '',
              status: u.status || 'Verificado',
              linkedAt: parsedDate,
              roleType: u.roleType || u.role_type,
              roleValue: u.roleValue || u.role_value,
              faceit_level: u.faceit_level || u.level,
              faceit_level_season: u.faceit_level_season,
              wallet_balance: u.wallet_balance || u.balance || 0,
              steam_id_64: u.steam_id_64 || u.steamId || ''
            };
          }));
        } catch (err) {
          console.error('Error parsing users data:', err);
        }
      }

      if (matchesRes) {
        try {
          const data = await matchesRes.json();
          const mData = data.matches || [];
          setMatches(mData.map((m: any) => ({
            id: m.id || m.match_id || '',
            teamAName: m.teamAName || m.teamA?.name || 'Equipo A',
            teamALogo: m.teamALogo || m.teamA?.avatar || '',
            teamBName: m.teamBName || m.teamB?.name || 'Equipo B',
            teamBLogo: m.teamBLogo || m.teamB?.avatar || '',
            map: m.map || 'TBD',
            location: m.location || 'EU',
            startedAtMinutesAgo: m.startedAtMinutesAgo || (m.startedAt ? Math.round((Date.now() - m.startedAt) / 60000) : 0),
            startedAt: m.startedAt
          })));
          if (data.stats) {
            setMatchStats(data.stats);
          }
        } catch (err) {
          console.error('Error parsing matches data:', err);
        }
      }

      if (logsRes) {
        try {
          setLogs((await logsRes.json()).logs || []);
        } catch (err) {
          console.error('Error parsing logs data:', err);
        }
      }

      if (settingsRes) {
        try {
          const s = (await settingsRes.json()).settings || {};
          setConfig(s);
          setFeatureFlags(buildFeatureFlags(s));
        } catch (err) {
          console.error('Error parsing settings data:', err);
        }
      }

      if (leaderboardRes) {
        try {
          const lData = await leaderboardRes.json();
          setLeaderboard(lData.leaderboard || []);
        } catch (err) {
          console.error('Error parsing leaderboard data:', err);
        }
      }

      if (systemRes) {
        try {
          const sData = await systemRes.json();
          setSystemInfo(sData);
        } catch (err) {
          console.error('Error parsing system data:', err);
        }
      }

      if (supportRes) {
        try {
          const sCalls = (await supportRes.json()).calls || [];
          setSupportCalls(sCalls);
        } catch (err) {
          console.error('Error parsing support calls:', err);
        }
      }

      if (messagesRes) {
        try {
          const templates = (await messagesRes.json()).templates || {};
          setMessageTemplates(templates);
        } catch (err) {
          console.error('Error parsing message templates:', err);
        }
      }

      if (historyRes) {
        try {
          const hData = (await historyRes.json()).history || [];
          setMatchHistory(hData);
        } catch (err) {
          console.error('Error parsing match history:', err);
        }
      }

      if (rolesRes) {
        try {
          const rData = await rolesRes.json();
          setAvailableDiscordRoles(rData.roles || []);
        } catch (err) {
          console.error('Error parsing discord roles:', err);
        }
      }

      if (channelsRes) {
        try {
          setDiscordChannels((await channelsRes.json()).channels || []);
        } catch (err) {
          console.error('Error parsing discord channels:', err);
        }
      }

      if (claimsRes) {
        try {
          const cData = (await claimsRes.json()).claims || [];
          setClaims(cData);
        } catch (err) {
          console.error('Error parsing claims data:', err);
        }
      }

      if (ordersRes) {
        try {
          const oData = (await ordersRes.json()).orders || [];
          setOrders(oData);
        } catch (err) {
          console.error('Error parsing orders data:', err);
        }
      }

      if (yapeRes) {
        try {
          const yData = await yapeRes.json();
          setYapeData({
            transactions: yData.transactions || [],
            operations: yData.operations || [],
            auditLogs: yData.auditLogs || []
          });
        } catch (err) {
          console.error('Error parsing yape data:', err);
        }
      }
    } catch (err) {
      console.error('Error loading data:', err);
    }
  };

  useEffect(() => {
    if (!isLoggedIn) return;
    loadData();
  }, [isLoggedIn, selectedGuildId]);

  const [activeTab, setActiveTab] = useState<string>('dashboard');
  const [sidebarCollapsed, setSidebarCollapsed] = useState<boolean>(false);
  const [topSearch, setTopSearch] = useState<string>('');
  const [dashboardSubTab, setDashboardSubTab] = useState<'leads' | 'deals' | 'projects'>('leads');
  const [dashboardPeriod, setDashboardPeriod] = useState<string>('30d');
  const [piePeriod, setPiePeriod] = useState<string>('30d');
  const [dashboardMenuOpen, setDashboardMenuOpen] = useState<boolean>(true);
  const [dashboardTableType, setDashboardTableType] = useState<'users' | 'matches' | 'history'>('users');
  const [pieMetric, setPieMetric] = useState<'users' | 'matches' | 'finances'>('users');
  const [syncing, setSyncing] = useState<boolean>(false);
  const [toast, setToast] = useState<{ message: string; type: 'success' | 'error' | 'info' } | null>(null);
  const [steamGuardCode, setSteamGuardCode] = useState<string>('');
  const [sendingSteamGuardCode, setSendingSteamGuardCode] = useState<boolean>(false);
  const [requestingSteamGuardCode, setRequestingSteamGuardCode] = useState<boolean>(false);
  const [steamLoginStatus, setSteamLoginStatus] = useState<SteamLoginStatus>(defaultSteamLoginStatus);
  const steamLoginBusy = ['starting', 'verifying', 'reconnecting'].includes(steamLoginStatus.status);

  const [liveTime, setLiveTime] = useState<string>('14:24:08');
  useEffect(() => {
    const update = () => {
      const now = new Date();
      setLiveTime(now.toTimeString().split(' ')[0]);
    };
    update();
    const interval = setInterval(update, 1000);
    return () => clearInterval(interval);
  }, []);

  // Filter & Search States
  const [usersSearch, setUsersSearch] = useState<string>('');
  const [usersFilter, setUsersFilter] = useState<string>('all');
  const [logsFilter, setLogsFilter] = useState<string>('all');
  const [statsSearch, setStatsSearch] = useState<string>('');
  const [featuresSearch, setFeaturesSearch] = useState<string>('');
  const [featuresCategory, setFeaturesCategory] = useState<'all' | 'modules' | 'commands'>('all');
  const [featuresStatusFilter, setFeaturesStatusFilter] = useState<'all' | 'active' | 'standby'>('all');
  const [featuresDomainFilter, setFeaturesDomainFilter] = useState<'all' | FeatureCategory>('all');
  const [featuresViewMode, setFeaturesViewMode] = useState<'grouped' | 'list'>('grouped');
  const [togglingFeatureKey, setTogglingFeatureKey] = useState<string | null>(null);
  const [batchUpdatingFeatures, setBatchUpdatingFeatures] = useState<boolean>(false);
  const [refreshingFeatures, setRefreshingFeatures] = useState<boolean>(false);

  // CS2 Map Pool / Veto States
  const [mapCatalog, setMapCatalog] = useState<MapCatalogItem[]>([]);
  const [loadingMapCatalog, setLoadingMapCatalog] = useState<boolean>(false);
  const [savingMapCatalog, setSavingMapCatalog] = useState<boolean>(false);
  const [newCustomMap, setNewCustomMap] = useState({ id: '', name: '', image: '', icon: '' });
  const [showAddMapModal, setShowAddMapModal] = useState<boolean>(false);
  
  // Pagination & Simulation States
  const [usersPage, setUsersPage] = useState<number>(1);
  const [simulatedEvent, setSimulatedEvent] = useState<string>('match_status_configuring');
  const [simMatchId, setSimMatchId] = useState<string>('sim_match_id_1234');
  const [simMap, setSimMap] = useState<string>('de_mirage');
  const [simTeamA, setSimTeamA] = useState<string>('Los Proscritos');
  const [simTeamB, setSimTeamB] = useState<string>('Los Vengadores');
  const [simWinner, setSimWinner] = useState<string>('faction1');
  const [simScoreA, setSimScoreA] = useState<number>(13);
  const [simScoreB, setSimScoreB] = useState<number>(7);
  const [simulatingWebhook, setSimulatingWebhook] = useState<boolean>(false);
  const [matchHistory, setMatchHistory] = useState<any[]>([]);
  const [broadcastMessage, setBroadcastMessage] = useState<string>('');
  const [broadcastImageUrl, setBroadcastImageUrl] = useState<string>('');
  const [broadcastTarget, setBroadcastTarget] = useState<{ discordId: string; faceitNickname: string } | null>(null);
  const [broadcastTargetSearch, setBroadcastTargetSearch] = useState<string>('');
  const [broadcastStatus, setBroadcastStatus] = useState<{ status: 'idle' | 'sending' | 'success' | 'error'; msg: string }>({ status: 'idle', msg: '' });
  const [activePlayer, setActivePlayer] = useState<any>(null);

  // Modals States
  const [verifyModalOpen, setVerifyModalOpen] = useState<boolean>(false);
  const [roleModalOpen, setRoleModalOpen] = useState<boolean>(false);
  const [roleTargetDiscordId, setRoleTargetDiscordId] = useState<string>('');

  // Admin web role states
  const [adminRoleModalOpen, setAdminRoleModalOpen] = useState<boolean>(false);
  const [adminRoleTargetUser, setAdminRoleTargetUser] = useState<any>(null);
  const [adminRoleValue, setAdminRoleValue] = useState<'usuario' | 'admin' | 'global_admin'>('usuario');
  const [adminRoleServer, setAdminRoleServer] = useState<string>('');

  // Form States (Manual Verification)
  const [manualDiscordId, setManualDiscordId] = useState<string>('');
  const [manualNickname, setManualNickname] = useState<string>('');

  // Form States (Assign Role)
  const [assignRoleType, setAssignRoleType] = useState<'level' | 'special'>('level');
  const [assignRoleValue, setAssignRoleValue] = useState<string>('');

  // Match History Action States
  const [selectedMatchStatsModal, setSelectedMatchStatsModal] = useState<any | null>(null);
  const [matchStatsDetails, setMatchStatsDetails] = useState<any | null>(null);
  const [loadingMatchStats, setLoadingMatchStats] = useState<boolean>(false);
  const [downloadingDemoId, setDownloadingDemoId] = useState<string | null>(null);
  const [matchToDelete, setMatchToDelete] = useState<any | null>(null);
  const [deletingMatchId, setDeletingMatchId] = useState<string | null>(null);

  // Reset users table pagination to page 1 when search or filters change
  useEffect(() => {
    setUsersPage(1);
  }, [usersSearch, usersFilter]);

  useEffect(() => {
    setClaimsPage(1);
  }, [claimsSearch, claimsFilter]);

  useEffect(() => {
    setOrdersPage(1);
  }, [ordersSearch, ordersFilter]);

  const formatDateDisplay = (dateVal: any) => {
    if (!dateVal) return 'N/A';
    try {
      if (typeof dateVal === 'number') {
        return new Date(dateVal).toLocaleString('es-ES');
      }
      if (dateVal._seconds) {
        return new Date(dateVal._seconds * 1000).toLocaleString('es-ES');
      }
      return new Date(dateVal).toLocaleString('es-ES');
    } catch {
      return String(dateVal);
    }
  };

  useEffect(() => {
    if (!isLoggedIn || activeTab !== 'logs') return;
    const fetchLogs = async () => {
      try {
        const guildQuery = selectedGuildId ? `?guildId=${selectedGuildId}` : '';
        const res = await apiFetch(`/api/admin/logs${guildQuery}`);
        if (res.ok) {
          setLogs((await res.json()).logs || []);
        }
      } catch (e) {
        console.warn('Failed to poll logs:', e);
      }
    };
    fetchLogs();
  }, [isLoggedIn, activeTab, selectedGuildId]);


  // --- Handlers para Gestión de Saldo / Billetera ---
  const handleOpenBalanceModal = (user: LinkedUser) => {
    setBalanceTargetUser(user);
    setBalanceActionType('add');
    setBalanceAmount('');
    setBalanceReason('Ajuste administrativo desde panel');
    setBalanceModalOpen(true);
  };

  const handleSaveBalanceSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!balanceTargetUser) return;
    const amountVal = parseFloat(balanceAmount);
    if (isNaN(amountVal) || amountVal < 0) {
      showToast('Ingresa un monto numérico válido.', 'error');
      return;
    }

    setSavingBalance(true);
    try {
      const res = await apiFetch(`/api/admin/users/${balanceTargetUser.discordId}/balance`, {
        method: 'POST',
        body: JSON.stringify({
          action: balanceActionType,
          amount: amountVal,
          reason: balanceReason.trim() || 'Ajuste administrativo'
        })
      });
      const data = await res.json().catch(() => ({}));
      if (res.ok && data.success) {
        showToast(data.message || 'Saldo actualizado correctamente.', 'success');
        setBalanceModalOpen(false);
        await loadData();
      } else {
        showToast(data.error || 'Error al actualizar saldo.', 'error');
      }
    } catch {
      showToast('Error de red al actualizar saldo.', 'error');
    } finally {
      setSavingBalance(false);
    }
  };

  // --- Handlers para Reclamaciones INDECOPI ---
  const handleOpenClaimModal = (claim: ClaimItem) => {
    setSelectedClaimModal(claim);
    setClaimResponseText(claim.response || '');
    setClaimResponseStatus((claim.status === 'PENDIENTE' ? 'ATENDIDO' : claim.status) as any);
  };

  const handleRespondClaimSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!selectedClaimModal) return;
    if (!claimResponseText.trim()) {
      showToast('Ingresa el texto formal de la respuesta.', 'error');
      return;
    }

    setRespondingClaim(true);
    try {
      const claimCode = selectedClaimModal.claimCode || selectedClaimModal.id;
      const res = await apiFetch(`/api/admin/claims/${claimCode}/respond`, {
        method: 'POST',
        body: JSON.stringify({
          status: claimResponseStatus,
          response: claimResponseText.trim()
        })
      });
      const data = await res.json().catch(() => ({}));
      if (res.ok && data.success) {
        showToast('Respuesta oficial registrada correctamente.', 'success');
        setSelectedClaimModal(null);
        await loadData();
      } else {
        showToast(data.error || 'Error al guardar respuesta.', 'error');
      }
    } catch {
      showToast('Error de red al responder reclamación.', 'error');
    } finally {
      setRespondingClaim(false);
    }
  };

  const handleDeleteClaim = async (claimCode: string) => {
    if (!confirm(`¿Eliminar la reclamación ${claimCode}? Esta acción es irreversible.`)) return;
    try {
      const res = await apiFetch(`/api/admin/claims/${claimCode}`, { method: 'DELETE' });
      if (res.ok) {
        showToast('Reclamación eliminada.', 'success');
        if (selectedClaimModal?.claimCode === claimCode) setSelectedClaimModal(null);
        await loadData();
      } else {
        showToast('Error al eliminar reclamación.', 'error');
      }
    } catch {
      showToast('Error de red al eliminar.', 'error');
    }
  };

  // --- Handlers para Órdenes de Tienda ---
  const handleOpenOrderModal = (order: OrderItem) => {
    setSelectedOrderModal(order);
  };

  const handleUpdateOrderStatus = async (orderId: string, status: string, note = '') => {
    setUpdatingOrderStatus(true);
    try {
      const res = await apiFetch(`/api/admin/orders/${orderId}/status`, {
        method: 'POST',
        body: JSON.stringify({ status, note })
      });
      const data = await res.json().catch(() => ({}));
      if (res.ok && data.success) {
        showToast(data.message || `Orden actualizada a ${status}.`, 'success');
        if (selectedOrderModal && selectedOrderModal.orderId === orderId) {
          setSelectedOrderModal({ ...selectedOrderModal, status: status as any, adminNote: note });
        }
        await loadData();
      } else {
        showToast(data.error || 'Error al actualizar orden.', 'error');
      }
    } catch {
      showToast('Error de red al actualizar estado de la orden.', 'error');
    } finally {
      setUpdatingOrderStatus(false);
    }
  };

  // --- Handlers para Validación Manual de Yape ---
  const handleOpenManualApproveYape = (item?: { op?: string; discordId?: string; amount?: number }) => {
    setManualApproveOp(item?.op || '');
    setManualApproveDiscordId(item?.discordId || '');
    setManualApproveAmount(item?.amount ? String(item.amount) : '');
    setManualApproveReason('Aprobación manual por verificación de comprobante');
    setManualApproveModalOpen(true);
  };

  const handleManualApproveSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    const val = parseFloat(manualApproveAmount);
    if (!manualApproveDiscordId.trim()) {
      showToast('El Discord ID es requerido.', 'error');
      return;
    }
    if (isNaN(val) || val <= 0) {
      showToast('Ingresa un monto válido mayor a 0.', 'error');
      return;
    }
    if (!manualApproveOp.trim()) {
      showToast('El número de operación es requerido.', 'error');
      return;
    }

    setApprovingYape(true);
    try {
      const res = await apiFetch('/api/admin/yape/manual-approve', {
        method: 'POST',
        body: JSON.stringify({
          discordId: manualApproveDiscordId.trim(),
          amount: val,
          operationNumber: manualApproveOp.trim(),
          reason: manualApproveReason.trim() || 'Aprobación manual de comprobante'
        })
      });
      const data = await res.json().catch(() => ({}));
      if (res.ok && data.success) {
        showToast(data.message || 'Depósito Yape aprobado y saldo acreditado.', 'success');
        setManualApproveModalOpen(false);
        await loadData();
      } else {
        showToast(data.error || 'Error al aprobar depósito.', 'error');
      }
    } catch {
      showToast('Error de red al aprobar depósito.', 'error');
    } finally {
      setApprovingYape(false);
    }
  };

  // Handle Login Flow
  const handleLogin = () => {
    window.location.href = '/auth/discord';
  };

  const handleLogout = () => {
    window.location.href = '/auth/logout';
  };

  const refreshSteamLoginStatus = async () => {
    try {
      const res = await apiFetch('/api/admin/steam/status');
      const data = await res.json().catch(() => ({}));
      if (res.ok && data.status) {
        setSteamLoginStatus(data.status);
      }
    } catch {
      // El estado se vuelve a consultar en el siguiente intervalo.
    }
  };

  const handleSubmitSteamGuardCode = async (event: React.FormEvent) => {
    event.preventDefault();
    if (sendingSteamGuardCode) return;

    const code = steamGuardCode.trim().toUpperCase();
    if (!/^[A-Z0-9]{5}$/.test(code)) {
      showToast('Ingresa el código de Steam Guard de 5 caracteres.', 'error');
      return;
    }

    setSendingSteamGuardCode(true);
    try {
      const res = await apiFetch('/api/admin/steam/guard', {
        method: 'POST',
        body: JSON.stringify({ code })
      });
      const data = await res.json().catch(() => ({}));
      if (!res.ok) {
        if (data.status) setSteamLoginStatus(data.status);
        showToast(data.error || 'No se pudo enviar el código a Steam.', 'error');
        return;
      }

      setSteamGuardCode('');
      if (data.status) setSteamLoginStatus(data.status);
      showToast(data.message || 'Código entregado a Steam. Validando...', 'info');
    } catch {
      showToast('Error de conexión al enviar el código a Steam.', 'error');
    } finally {
      setSendingSteamGuardCode(false);
    }
  };

  const [resettingSteamCooldown, setResettingSteamCooldown] = useState<boolean>(false);

  const handleResetSteamCooldown = async () => {
    if (resettingSteamCooldown) return;
    setResettingSteamCooldown(true);
    try {
      const res = await apiFetch('/api/admin/steam/reset', { method: 'POST' });
      const data = await res.json().catch(() => ({}));
      if (data.status) setSteamLoginStatus(data.status);
      showToast(data.message || 'Cooldown reiniciado con éxito.', res.ok ? 'success' : 'error');
    } catch {
      showToast('Error al reiniciar cooldown de Steam.', 'error');
    } finally {
      setResettingSteamCooldown(false);
    }
  };

  const handleRequestSteamGuardCode = async () => {
    if (requestingSteamGuardCode) return;

    setRequestingSteamGuardCode(true);
    try {
      const res = await apiFetch('/api/admin/steam/login', { method: 'POST' });
      const data = await res.json().catch(() => ({}));
      if (data.status) setSteamLoginStatus(data.status);
      showToast(data.message || data.error || 'No se pudo iniciar Steam Guard.', res.ok ? 'info' : 'error');
    } catch {
      showToast('Error de conexión al iniciar Steam Guard.', 'error');
    } finally {
      setRequestingSteamGuardCode(false);
    }
  };

  useEffect(() => {
    if (!isLoggedIn || activeTab !== 'config' || currentUser?.role !== 'global_admin') return;
    refreshSteamLoginStatus();
    const interval = window.setInterval(refreshSteamLoginStatus, 5000);
    return () => window.clearInterval(interval);
  }, [isLoggedIn, activeTab, currentUser?.role]);



  // Trigger Sincronizar Data Real
  const [syncingNicks, setSyncingNicks] = useState(false);

  const handleSyncData = async () => {
    if (syncing) return;
    setSyncing(true);
    showToast('Sincronizando todos los usuarios con Faceit (roles y apodos)...', 'info');
    try {
      const guildQuery = selectedGuildId ? `?guildId=${selectedGuildId}` : '';
      const res = await apiFetch(`/api/admin/users/sync-all${guildQuery}`, {
        method: 'POST'
      });
      const data = await res.json();
      if (res.ok && data.success) {
        showToast(data.message || 'Sincronización completada con éxito.', 'success');
        await loadData();
      } else {
        showToast(data.error || 'Error al sincronizar usuarios.', 'error');
      }
    } catch (err) {
      showToast('Error de red al sincronizar.', 'error');
    } finally {
      setSyncing(false);
    }
  };

  const handleSyncAllNicks = async () => {
    if (syncingNicks) return;
    setSyncingNicks(true);
    showToast('Sincronizando apodos de Discord para todos los usuarios...', 'info');
    try {
      const guildQuery = selectedGuildId ? `?guildId=${selectedGuildId}` : '';
      const res = await apiFetch(`/api/admin/users/sync-nicks${guildQuery}`, {
        method: 'POST'
      });
      const data = await res.json();
      if (res.ok && data.success) {
        showToast(data.message || 'Sincronización de apodos iniciada correctamente.', 'success');
      } else {
        showToast(data.error || 'Error al sincronizar apodos.', 'error');
      }
    } catch (err) {
      showToast('Error de red al sincronizar apodos.', 'error');
    } finally {
      setSyncingNicks(false);
    }
  };

  const handleSyncUserNick = async (discordId: string, nickname: string) => {
    showToast(`Sincronizando apodo de ${nickname}...`, 'info');
    try {
      const guildQuery = selectedGuildId ? `?guildId=${selectedGuildId}` : '';
      const res = await apiFetch(`/api/admin/users/${discordId}/sync-nick${guildQuery}`, {
        method: 'POST'
      });
      const data = await res.json();
      if (res.ok && data.success) {
        showToast(data.message || 'Apodo sincronizado con éxito.', 'success');
      } else {
        showToast(data.error || 'No se pudo actualizar el apodo.', 'error');
      }
    } catch (err) {
      showToast('Error de red al sincronizar apodo.', 'error');
    }
  };

  const handleExportBackup = async () => {
    try {
      const res = await apiFetch('/api/admin/users/backup');
      if (res.ok) {
        const data = await res.json();
        const blob = new Blob([JSON.stringify(data, null, 2)], { type: 'application/json' });
        const url = URL.createObjectURL(blob);
        const a = document.createElement('a');
        a.href = url;
        a.download = `backup-faceit-users-${new Date().toISOString().split('T')[0]}.json`;
        document.body.appendChild(a);
        a.click();
        document.body.removeChild(a);
        URL.revokeObjectURL(url);
        showToast('Backup exportado correctamente.', 'success');
      } else {
        showToast('Error al exportar backup.', 'error');
      }
    } catch {
      showToast('Error de red al exportar.', 'error');
    }
  };

  const handleImportBackup = async (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (!file) return;

    if (!confirm('¿Estás seguro de importar este backup? Esto podría sobrescribir o añadir usuarios a la base de datos.')) {
      e.target.value = '';
      return;
    }

    const reader = new FileReader();
    reader.onload = async (event) => {
      try {
        const usersData = JSON.parse(event.target?.result as string);
        if (!Array.isArray(usersData)) {
          showToast('El archivo de backup no tiene el formato correcto.', 'error');
          return;
        }

        const res = await apiFetch('/api/admin/users/restore', {
          method: 'POST',
          body: JSON.stringify({ users: usersData }),
        });
        const result = await res.json();
        if (res.ok && result.success) {
          showToast(result.message || 'Backup importado correctamente.', 'success');
          await loadData();
        } else {
          showToast(result.error || 'Error al importar backup.', 'error');
        }
      } catch (err) {
        showToast('Error al procesar la importación.', 'error');
      }
    };
    reader.readAsText(file);
    e.target.value = ''; // Reset input
  };

  // Manual User Verification Action
  const handleManualVerify = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!manualDiscordId.trim() || !manualNickname.trim()) {
      showToast('Por favor completa todos los campos.', 'error');
      return;
    }

    showToast('Enviando solicitud de verificación...', 'info');
    try {
      const guildQuery = selectedGuildId ? `?guildId=${selectedGuildId}` : '';
      const res = await apiFetch(`/api/admin/users/verify${guildQuery}`, {
        method: 'POST',
        body: JSON.stringify({
          discordId: manualDiscordId.trim(),
          faceitNickname: manualNickname.trim()
        })
      });
      const data = await res.json();
      if (res.ok && data.success) {
        showToast('Usuario verificado y vinculado exitosamente.', 'success');
        setVerifyModalOpen(false);
        setManualDiscordId('');
        setManualNickname('');
        await loadData();
      } else {
        showToast(data.error || 'Error al verificar usuario.', 'error');
      }
    } catch (err) {
      showToast('Error de red al verificar usuario.', 'error');
    }
  };

  // Unlink user account
  const handleUnlinkUser = async (discordId: string) => {
    const targetUser = users.find(u => u.discordId === discordId);
    if (!targetUser) return;

    if (window.confirm(`¿Seguro que deseas desvincular al usuario ${targetUser.faceitNickname}?`)) {
      try {
        const guildQuery = selectedGuildId ? `?guildId=${selectedGuildId}` : '';
        const res = await apiFetch(`/api/admin/users/${discordId}/unlink${guildQuery}`, {
          method: 'POST'
        });
        const data = await res.json();
        if (res.ok && data.success) {
          showToast('Usuario desvinculado con éxito.', 'success');
          await loadData();
        } else {
          showToast(data.error || 'Error al desvincular usuario.', 'error');
        }
      } catch (err) {
        showToast('Error de red al desvincular.', 'error');
      }
    }
  };

  // Assign custom role type & value
  const handleAssignRoleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!assignRoleValue.trim()) {
      showToast('Por favor ingresa un valor para el rol.', 'error');
      return;
    }

    try {
      const guildQuery = selectedGuildId ? `?guildId=${selectedGuildId}` : '';
      const res = await apiFetch(`/api/admin/users/${roleTargetDiscordId}/roles${guildQuery}`, {
        method: 'POST',
        body: JSON.stringify({
          roleType: assignRoleType,
          roleValue: assignRoleValue.trim()
        })
      });
      const data = await res.json();
      if (res.ok && data.success) {
        showToast('Rol asignado correctamente.', 'success');
        setRoleModalOpen(false);
        setAssignRoleValue('');
        await loadData();
      } else {
        showToast(data.error || 'Error al asignar rol.', 'error');
      }
    } catch (err) {
      showToast('Error de red al asignar rol.', 'error');
    }
  };

  const handleOpenRoleModal = (discordId: string) => {
    setRoleTargetDiscordId(discordId);
    const user = users.find(u => u.discordId === discordId);
    setAssignRoleType(user?.roleType || 'level');
    setAssignRoleValue(user?.roleValue || '');
    setRoleModalOpen(true);
  };

  const handleOpenAdminRoleModal = (user: any) => {
    setAdminRoleTargetUser(user);
    setAdminRoleValue(user.rol || 'usuario');
    setAdminRoleServer(user.servidor || '');
    setAdminRoleModalOpen(true);
  };

  const handleSaveAdminRoleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!adminRoleTargetUser) return;
    try {
      const res = await apiFetch(`/api/admin/users/${adminRoleTargetUser.discordId}/admin-role`, {
        method: 'POST',
        body: JSON.stringify({
          role: adminRoleValue,
          guildId: adminRoleValue === 'admin' ? adminRoleServer : null
        })
      });
      if (res.ok) {
        showToast('¡Permisos actualizados correctamente!', 'success');
        setAdminRoleModalOpen(false);
        await loadData();
      } else {
        const data = await res.json().catch(() => ({}));
        showToast(data.error || 'Error al actualizar permisos.', 'error');
      }
    } catch {
      showToast('Error de conexión al actualizar permisos.', 'error');
    }
  };

  const handleAnnounceMixtonMatch = async (matchId: string) => {
    try {
      const res = await apiFetch(`/api/admin/matches/${matchId}/announce-mixton`, {
        method: 'POST'
      });
      const data = await res.json().catch(() => ({}));
      if (res.ok && data.success) {
        showToast(data.isActive ? 'Anuncio enviado con botones de resultado.' : 'Resumen de historial enviado.', 'success');
        await loadData();
      } else {
        showToast(data.error || 'No se pudo enviar el anuncio.', 'error');
      }
    } catch {
      showToast('Error de red al enviar anuncio.', 'error');
    }
  };

  // Cleanup active live match
  const handleCleanupMatch = async (matchId: string) => {
    if (window.confirm(`¿Seguro que deseas forzar la limpieza de la partida ${matchId}?`)) {
      try {
        const res = await apiFetch(`/api/admin/matches/${matchId}/cleanup`, {
          method: 'POST'
        });
        const data = await res.json();
        if (res.ok && data.success) {
          showToast(`Partida ${matchId} limpiada correctamente.`, 'success');
          await loadData();
        } else {
          showToast(data.error || 'Error al limpiar partida.', 'error');
        }
      } catch (err) {
        showToast('Error de red al limpiar partida.', 'error');
      }
    }
  };

  // Handlers para acciones del Historial de Partidas
  const handleDownloadMatchDemo = async (m: any) => {
    if (m.demo_url) {
      window.open(m.demo_url, '_blank');
      return;
    }
    setDownloadingDemoId(m.id);
    showToast('Obteniendo enlace de descarga de la partida...', 'info');
    try {
      const res = await apiFetch(`/api/admin/matches/${m.id}/demo`);
      const data = await res.json();
      if (res.ok && data.demo_url) {
        window.open(data.demo_url, '_blank');
        showToast('Iniciando descarga de la demo.', 'success');
      } else if (data.faceit_url) {
        showToast('Demo en proceso o no lista. Abriendo sala en Faceit.', 'info');
        window.open(data.faceit_url, '_blank');
      } else {
        showToast('Esta partida no cuenta con archivo de demo disponible.', 'info');
      }
    } catch (err) {
      showToast('Error de red al intentar descargar la demo.', 'error');
    } finally {
      setDownloadingDemoId(null);
    }
  };

  const handleViewMatchStats = async (m: any) => {
    setSelectedMatchStatsModal(m);
    setMatchStatsDetails(null);
    setLoadingMatchStats(true);

    if (!m.id.startsWith('mixton_') && !m.id.startsWith('manual_')) {
      try {
        const res = await apiFetch(`/api/admin/matches/${m.id}/stats`);
        if (res.ok) {
          const data = await res.json();
          if (data.success && data.stats) {
            setMatchStatsDetails(data.stats);
          }
        }
      } catch (err) {
        console.error('Error al obtener estadísticas detalladas:', err);
      }
    }
    setLoadingMatchStats(false);
  };

  const handleDeleteMatchHistory = async () => {
    if (!matchToDelete) return;
    const matchId = matchToDelete.id;
    setDeletingMatchId(matchId);
    try {
      const res = await apiFetch(`/api/admin/matches/history/${matchId}`, {
        method: 'DELETE'
      });
      const data = await res.json();
      if (res.ok && data.success) {
        setMatchHistory(prev => prev.filter(m => m.id !== matchId));
        showToast(`Partida ${matchId} eliminada del historial con éxito.`, 'success');
        setMatchToDelete(null);
      } else {
        showToast(data.error || 'Error al eliminar partida del historial.', 'error');
      }
    } catch (err) {
      showToast('Error de red al eliminar partida del historial.', 'error');
    } finally {
      setDeletingMatchId(null);
    }
  };

  // Feature Categories Metadata for Visual Grouping
  const CATEGORY_META = {
    voice_music: {
      label: 'Música & Canales de Voz',
      badge: 'AUDIO & VOZ',
      desc: 'Bot de música interactivo de YouTube, canales temporales automáticos y movimiento de jugadores.',
      icon: Music,
      color: 'text-purple-400',
      borderColor: 'border-purple-500/30',
      bgColor: 'bg-purple-500/10',
      accentBg: 'bg-purple-500/20',
      badgeBg: 'bg-purple-500/10 text-purple-300 border-purple-500/20',
    },
    faceit_matches: {
      label: 'FACEIT & Partidas en Vivo',
      badge: 'COMPETITIVO CS2',
      desc: 'Anuncios automáticos, panel fijado en vivo, estado de servidores LATAM y armado de salas 5v5.',
      icon: Flame,
      color: 'text-emerald-400',
      borderColor: 'border-emerald-500/30',
      bgColor: 'bg-emerald-500/10',
      accentBg: 'bg-emerald-500/20',
      badgeBg: 'bg-emerald-500/10 text-emerald-300 border-emerald-500/20',
    },
    community_roles: {
      label: 'Perfiles, Roles & Comunidad',
      badge: 'COMUNIDAD & ELO',
      desc: 'Verificación de cuentas FACEIT, asignación de roles de nivel (1-10), apodos y ranking por puntos.',
      icon: Users,
      color: 'text-blue-400',
      borderColor: 'border-blue-500/30',
      bgColor: 'bg-blue-500/10',
      accentBg: 'bg-blue-500/20',
      badgeBg: 'bg-blue-500/10 text-blue-300 border-blue-500/20',
    },
    mixton_queues: {
      label: 'Mixton 5v5 & Colas',
      badge: 'COLAS MIXTON',
      desc: 'Panel fijado de jugadores listos y comandos /listo y /nolisto para partidas mixtas.',
      icon: Swords,
      color: 'text-amber-400',
      borderColor: 'border-amber-500/30',
      bgColor: 'bg-amber-500/10',
      accentBg: 'bg-amber-500/20',
      badgeBg: 'bg-amber-500/10 text-amber-300 border-amber-500/20',
    },
    moderation_support: {
      label: 'Moderación, Alertas & Soporte',
      badge: 'STAFF & SEGURIDAD',
      desc: 'Llamados de auxilio en partida activa, avisos automáticos de sanciones de Hub y comunicados oficiales.',
      icon: ShieldAlert,
      color: 'text-red-400',
      borderColor: 'border-red-500/30',
      bgColor: 'bg-red-500/10',
      accentBg: 'bg-red-500/20',
      badgeBg: 'bg-red-500/10 text-red-300 border-red-500/20',
    },
    wallet_economy: {
      label: 'Billetera & Recargas Yape',
      badge: 'FINANZAS & APUESTAS',
      desc: 'Validación de transferencias Yape, saldo de billetera virtual y paneles de recarga instantánea.',
      icon: Wallet,
      color: 'text-pink-400',
      borderColor: 'border-pink-500/30',
      bgColor: 'bg-pink-500/10',
      accentBg: 'bg-pink-500/20',
      badgeBg: 'bg-pink-500/10 text-pink-300 border-pink-500/20',
    },
  } as const;

  // Filtered Feature Flags
  const filteredFeatureFlags = useMemo(() => {
    return featureFlags.filter(flag => {
      // Type Filter
      if (featuresCategory === 'modules' && flag.type !== 'module') return false;
      if (featuresCategory === 'commands' && flag.type !== 'command') return false;
      // Status Filter
      if (featuresStatusFilter === 'active' && !flag.enabled) return false;
      if (featuresStatusFilter === 'standby' && flag.enabled) return false;
      // Domain Category Filter
      if (featuresDomainFilter !== 'all' && flag.category !== featuresDomainFilter) return false;
      // Search
      if (featuresSearch.trim()) {
        const query = featuresSearch.toLowerCase();
        return (
          flag.name.toLowerCase().includes(query) ||
          flag.desc.toLowerCase().includes(query) ||
          flag.key.toLowerCase().includes(query) ||
          (flag.channelLabel && flag.channelLabel.toLowerCase().includes(query))
        );
      }
      return true;
    });
  }, [featureFlags, featuresCategory, featuresStatusFilter, featuresDomainFilter, featuresSearch]);

  // Statistics for KPIs
  const featureStats = useMemo(() => {
    const total = featureFlags.length;
    const active = featureFlags.filter(f => f.enabled).length;
    const standby = total - active;
    const modules = featureFlags.filter(f => f.type === 'module');
    const modulesActive = modules.filter(f => f.enabled).length;
    const commands = featureFlags.filter(f => f.type === 'command');
    const commandsActive = commands.filter(f => f.enabled).length;

    const channelsTotal = featureFlags.filter(f => !!f.channelKey).length;
    const channelsAssigned = featureFlags.filter(f => {
      if (!f.channelKey) return false;
      const val = (config as any)?.[f.channelKey];
      return val && String(val).trim() !== '' && !String(val).startsWith('ID_');
    }).length;

    const percent = total > 0 ? Math.round((active / total) * 100) : 0;

    return {
      total,
      active,
      standby,
      percent,
      modulesTotal: modules.length,
      modulesActive,
      commandsTotal: commands.length,
      commandsActive,
      channelsTotal,
      channelsAssigned
    };
  }, [featureFlags, config]);

  // Batch toggle features handler
  const handleBatchToggleFeatures = async (action: 'enable_all' | 'disable_all' | 'toggle_category', targetCategory?: string) => {
    if (batchUpdatingFeatures) return;
    setBatchUpdatingFeatures(true);
    try {
      const targetFlags = featureFlags.filter(f => {
        if (targetCategory && f.category !== targetCategory) return false;
        return true;
      });

      if (targetFlags.length === 0) {
        showToast('No hay servicios que coincidan con la acción.', 'info');
        setBatchUpdatingFeatures(false);
        return;
      }

      const nextState = action === 'enable_all' ? true : action === 'disable_all' ? false : null;
      const updates: Record<string, boolean> = {};

      targetFlags.forEach(f => {
        updates[f.key] = nextState !== null ? nextState : !f.enabled;
      });

      const guildQuery = selectedGuildId ? `?guildId=${selectedGuildId}` : '';
      const res = await apiFetch(`/api/admin/features-batch${guildQuery}`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ updates })
      });

      if (res.ok) {
        setFeatureFlags(prev => prev.map(f => {
          if (updates[f.key] !== undefined) {
            return { ...f, enabled: updates[f.key] };
          }
          return f;
        }));
        setConfig(prev => ({ ...prev, ...updates }));
        showToast(`Servicios actualizados correctamente (${Object.keys(updates).length} modificados).`, 'success');
      } else {
        const data = await res.json().catch(() => ({}));
        showToast(data.error || 'Error al actualizar servicios por lote.', 'error');
      }
    } catch {
      showToast('Error de conexión al procesar cambio por lote.', 'error');
    } finally {
      setBatchUpdatingFeatures(false);
    }
  };

  // Update Feature Toggle States
  const handleToggleFeature = async (key: string) => {
    if (togglingFeatureKey) return;
    const flag = featureFlags.find(f => f.key === key);
    if (!flag) return;
    const nextState = !flag.enabled;
    setTogglingFeatureKey(key);
    try {
      const guildQuery = selectedGuildId ? `?guildId=${selectedGuildId}` : '';
      const res = await apiFetch(`/api/admin/features/${key}${guildQuery}`, {
        method: 'POST',
        body: JSON.stringify({ enabled: nextState }),
      });
      const data = await res.json().catch(() => ({}));
      if (res.ok && data.success) {
        const resolvedState = data.enabled !== undefined ? data.enabled : nextState;
        setFeatureFlags(prev => prev.map(f => f.key === key ? { ...f, enabled: resolvedState } : f));
        setConfig(prev => ({ ...prev, [key]: resolvedState }));
        showToast(`Servicio "${flag.name}" ${resolvedState ? 'activado' : 'desactivado'}.`, 'success');
      } else {
        showToast(data.error || 'Error al cambiar el servicio.', 'error');
      }
    } catch {
      showToast('Error de conexión al cambiar el servicio.', 'error');
    } finally {
      setTogglingFeatureKey(null);
    }
  };

  // Re-sincronizar y verificar estado de servicios en vivo desde la base de datos
  const handleRefreshFeatures = async () => {
    if (refreshingFeatures) return;
    setRefreshingFeatures(true);
    try {
      const url = selectedGuildId ? `/api/admin/guilds/${selectedGuildId}/settings` : '/api/admin/settings';
      const res = await apiFetch(url);
      if (res.ok) {
        const s = (await res.json()).settings || {};
        setConfig(s);
        setFeatureFlags(buildFeatureFlags(s));
        showToast('Servicios sincronizados correctamente con la base de datos.', 'success');
      } else {
        showToast('Error al consultar configuración de servicios en la base de datos.', 'error');
      }
    } catch {
      showToast('Error de conexión al sincronizar servicios.', 'error');
    } finally {
      setRefreshingFeatures(false);
    }
  };

  // Handlers para Multi-Cola Faceit
  const handleAddQueue = () => {
    const currentQueues = config.queues || [];
    const newQueue = {
      id: `queue-${Date.now()}`,
      queueId: '',
      name: `Cola ${currentQueues.length + 1}`,
      discordMatchesChannelId: '',
      discordWaitingRoomChannelId: '',
      discordMixtonChannelId: '',
      discordMixtonChatChannelId: '',
      discordMatchAccessRoleId: '',
      hubRoleId: '',
      discordHubRoleId: '',
    };
    setConfig({ ...config, queues: [...currentQueues, newQueue] });
  };

  const handleUpdateQueue = (index: number, field: string, value: string) => {
    const currentQueues = [...(config.queues || [])];
    if (currentQueues[index]) {
      currentQueues[index] = { ...currentQueues[index], [field]: value };
      if (field === 'queueId' && !currentQueues[index].id) {
        currentQueues[index].id = value;
      }
      setConfig({ ...config, queues: currentQueues });
    }
  };

  const handleRemoveQueue = (index: number) => {
    const currentQueues = (config.queues || []).filter((_, i) => i !== index);
    setConfig({ ...config, queues: currentQueues });
  };

  const [isSyncingFaceitQueues, setIsSyncingFaceitQueues] = useState(false);

  const handleSyncFaceitQueues = async () => {
    setIsSyncingFaceitQueues(true);
    try {
      const res = await apiFetch(`/api/admin/faceit-club-queues${selectedGuildId ? `?guildId=${selectedGuildId}` : ''}`);
      if (!res.ok) throw new Error('Error al consultar colas en Faceit');
      const data = await res.json();
      if (!data.queues || data.queues.length === 0) {
        showToast('No se encontraron colas adicionales en el Hub/Club de Faceit.', 'info');
        return;
      }
      const currentQueues = [...(config.queues || [])];
      let added = 0;
      for (const fq of data.queues) {
        const exists = currentQueues.some(cq => cq.queueId === fq.queueId || cq.id === fq.queueId);
        if (!exists) {
          currentQueues.push({
            id: fq.queueId,
            queueId: fq.queueId,
            name: fq.name,
            discordMatchesChannelId: '',
            discordWaitingRoomChannelId: '',
            discordMixtonChannelId: '',
            discordMixtonChatChannelId: '',
            discordMatchAccessRoleId: '',
            hubRoleId: '',
            discordHubRoleId: '',
          });
          added++;
        }
      }
      setConfig({ ...config, queues: currentQueues });
      showToast(`¡Sincronización completa! Se agregaron ${added} colas/divisiones de Faceit.`, 'success');
    } catch (err: any) {
      showToast(err.message || 'Error al sincronizar colas de Faceit', 'error');
    } finally {
      setIsSyncingFaceitQueues(false);
    }
  };

  // Handle Save Configuration
  const handleSaveConfig = async (e: React.FormEvent) => {
    e.preventDefault();
    try {
      const url = selectedGuildId ? `/api/admin/guilds/${selectedGuildId}/settings` : '/api/admin/settings';
      const res = await apiFetch(url, {
        method: 'POST',
        body: JSON.stringify(config),
      });
      if (res.ok) {
        showToast('¡Configuración guardada correctamente!', 'success');
      } else {
        showToast('Error al guardar la configuración.', 'error');
      }
    } catch {
      showToast('Error de conexión.', 'error');
    }
  };

  // --- CS2 Map Pool / Veto Management ---
  const fetchMapCatalog = async () => {
    setLoadingMapCatalog(true);
    try {
      const guildQuery = selectedGuildId ? `?guildId=${selectedGuildId}` : '';
      const res = await apiFetch(`/api/admin/map-catalog${guildQuery}`);
      if (res.ok) {
        const data = await res.json();
        if (Array.isArray(data.catalog)) {
          setMapCatalog(data.catalog);
        }
      }
    } catch (err) {
      console.error('Error fetching map catalog:', err);
    } finally {
      setLoadingMapCatalog(false);
    }
  };

  useEffect(() => {
    if (!isLoggedIn) return;
    if (activeTab === 'config') {
      fetchMapCatalog();
    }
  }, [isLoggedIn, activeTab, selectedGuildId]);

  const handleToggleMap = (mapId: string) => {
    setMapCatalog(prev => prev.map(m => m.id === mapId ? { ...m, active: !m.active } : m));
  };

  const handleApplyPreset = (preset: 'premier' | 'all' | 'classics' | 'fast') => {
    if (preset === 'premier') {
      const premierIds = ['de_mirage', 'de_inferno', 'de_dust2', 'de_nuke', 'de_ancient', 'de_anubis', 'de_vertigo'];
      setMapCatalog(prev => prev.map(m => ({
        ...m,
        active: premierIds.includes(m.id)
      })));
      showToast('Preset aplicado: Pool Oficial Premier (7 mapas)', 'info');
    } else if (preset === 'all') {
      setMapCatalog(prev => prev.map(m => ({ ...m, active: true })));
      showToast('Preset aplicado: Todos los mapas activos', 'info');
    } else if (preset === 'classics') {
      const classicIds = ['de_mirage', 'de_inferno', 'de_dust2', 'de_nuke', 'de_overpass'];
      setMapCatalog(prev => prev.map(m => ({
        ...m,
        active: classicIds.includes(m.id)
      })));
      showToast('Preset aplicado: 5 Clásicos (Mirage, Inferno, Dust II, Nuke, Overpass)', 'info');
    } else if (preset === 'fast') {
      const fastIds = ['de_mirage', 'de_inferno', 'de_dust2'];
      setMapCatalog(prev => prev.map(m => ({
        ...m,
        active: fastIds.includes(m.id)
      })));
      showToast('Preset aplicado: 3 Mapas Rápidos (Mirage, Inferno, Dust II)', 'info');
    }
  };

  const handleAddCustomMap = () => {
    if (!newCustomMap.id.trim() || !newCustomMap.name.trim()) {
      showToast('El ID y el Nombre del mapa son obligatorios.', 'error');
      return;
    }
    const cleanId = newCustomMap.id.trim().toLowerCase().replace(/[^a-z0-9_]/g, '_');
    if (mapCatalog.some(m => m.id === cleanId)) {
      showToast('Ya existe un mapa con ese ID.', 'error');
      return;
    }
    const item: MapCatalogItem = {
      id: cleanId,
      name: newCustomMap.name.trim(),
      image: newCustomMap.image.trim() || 'https://raw.githubusercontent.com/ggMartinez/CS2-Maps-Images/main/maps/de_mirage.png',
      icon: newCustomMap.icon.trim() || 'https://raw.githubusercontent.com/MurkyYT/cs2-map-icons/main/images/de_mirage.png',
      active: true,
      isCustom: true
    };
    setMapCatalog(prev => [...prev, item]);
    setNewCustomMap({ id: '', name: '', image: '', icon: '' });
    setShowAddMapModal(false);
    showToast(`Mapa ${item.name} añadido correctamente. Recuerda guardar cambios.`, 'success');
  };

  const handleDeleteCustomMap = (mapId: string) => {
    setMapCatalog(prev => prev.filter(m => m.id !== mapId));
    showToast('Mapa eliminado del catálogo.', 'info');
  };

  const handleSaveMapCatalog = async () => {
    const activeCount = mapCatalog.filter(m => m.active).length;
    if (activeCount < 2) {
      showToast('Debes mantener al menos 2 mapas activos para el sistema de veto.', 'error');
      return;
    }
    setSavingMapCatalog(true);
    try {
      const guildQuery = selectedGuildId ? `?guildId=${selectedGuildId}` : '';
      const res = await apiFetch(`/api/admin/map-catalog${guildQuery}`, {
        method: 'POST',
        body: JSON.stringify({ catalog: mapCatalog })
      });
      if (res.ok) {
        const data = await res.json();
        if (Array.isArray(data.catalog)) setMapCatalog(data.catalog);
        showToast('¡Configuración de mapas guardada correctamente!', 'success');
      } else {
        const errData = await res.json().catch(() => ({}));
        showToast(errData.error || 'Error al guardar mapas.', 'error');
      }
    } catch {
      showToast('Error de conexión al guardar mapas.', 'error');
    } finally {
      setSavingMapCatalog(false);
    }
  };

  // Trigger Simulated Faceit Webhook
  const handleTriggerSimulatedWebhook = async () => {
    if (simulatingWebhook) return;
    setSimulatingWebhook(true);
    showToast('Enviando evento webhook simulado al servidor...', 'info');

    // Usar los usuarios vinculados reales del club para simular los rosters
    const linkedUsers = users.slice(0, 10);
    const rosterA = linkedUsers.slice(0, 5).map(u => ({ id: u.faceitPlayerId, nickname: u.faceitNickname }));
    const rosterB = linkedUsers.slice(5, 10).map(u => ({ id: u.faceitPlayerId, nickname: u.faceitNickname }));

    // Completar con datos ficticios si la lista está incompleta
    while (rosterA.length < 5) {
      const idx = rosterA.length + 1;
      rosterA.push({ id: `mock_player_a_${idx}`, nickname: `PlayerA_${idx}` });
    }
    while (rosterB.length < 5) {
      const idx = rosterB.length + 1;
      rosterB.push({ id: `mock_player_b_${idx}`, nickname: `PlayerB_${idx}` });
    }

    const payload = {
      event: simulatedEvent,
      entity: {
        id: config.faceitHubId || ''
      },
      payload: {
        id: simMatchId,
        game: 'cs2',
        entity_custom_name: 'Club Test',
        voting: {
          map: { pick: [simMap] },
          location: { pick: ['lima'] }
        },
        teams: {
          faction1: {
            name: simTeamA,
            roster: rosterA.map(p => ({ player_id: p.id, nickname: p.nickname }))
          },
          faction2: {
            name: simTeamB,
            roster: rosterB.map(p => ({ player_id: p.id, nickname: p.nickname }))
          }
        },
        results: simulatedEvent === 'match_status_finished' ? {
          winner: simWinner,
          score: {
            faction1: simScoreA,
            faction2: simScoreB
          }
        } : null
      }
    };

    try {
      const res = await apiFetch('/api/admin/simulate-webhook', {
        method: 'POST',
        body: JSON.stringify(payload)
      });
      const data = await res.json();
      if (res.ok && data.success) {
        showToast(data.message || 'Webhook simulado enviado.', 'success');
        await loadData();
      } else {
        showToast(data.error || 'Error al enviar webhook simulado.', 'error');
      }
    } catch (err) {
      showToast('Error de conexión al simular webhook.', 'error');
    } finally {
      setSimulatingWebhook(false);
    }
  };

  // Stats player search simulation
  const handlePlayerSearch = async (e?: React.FormEvent, directSearch?: string) => {
    if (e) e.preventDefault();
    const query = directSearch || statsSearch.trim();
    if (!query) {
      showToast('Por favor introduce un nickname válido.', 'error');
      return;
    }

    try {
      const res = await apiFetch(`/api/admin/stats/${query}`);
      if (res.ok) {
        const data = await res.json();
        setActivePlayer({
          nickname: data.player?.nickname || query,
          country: data.player?.country || 'un',
          avatarUrl: data.player?.avatar || '',
          elo: data.player?.elo || 0,
          level: data.player?.level || 1,
          role: data.player?.role || `LVL ${data.player?.level || 1}`,
          totalMatches: data.stats?.lifetime?.Matches || 0,
          winRate: data.stats?.lifetime?.['Win Rate %'] || 0,
          kdRatio: data.stats?.lifetime?.['Average K/D Ratio'] || 0,
          globalRank: 'NO DISPONIBLE',
          countryRank: 'NO DISPONIBLE',
          recentForm: (data.stats?.lifetime?.['Recent Results'] || []).map((r: string) => r === '1' ? 'W' : 'L'),
          recentPoints: Array(10).fill(25), // Mock points since API doesn't provide them yet
          membership: 'FREE'
        });
        showToast(`Estadísticas de ${query} cargadas.`, 'success');
      } else {
        showToast('Jugador no encontrado', 'error');
        setActivePlayer(null);
      }
    } catch(e) {
      showToast('Error de conexión', 'error');
      setActivePlayer(null);
    }
  };

  // Filter linked users
  const filteredUsers = useMemo(() => {
    return users.filter(user => {
      const matchesSearch = user.faceitNickname.toLowerCase().includes(usersSearch.toLowerCase()) ||
        user.discordId.includes(usersSearch) ||
        user.faceitPlayerId.includes(usersSearch);
      
      if (usersFilter === 'all') return matchesSearch;
      if (usersFilter === 'Verificado') return matchesSearch && user.status === 'Verificado';
      if (usersFilter === 'Pendiente') return matchesSearch && user.status === 'Pendiente';
      if (usersFilter === 'Baneado') return matchesSearch && user.status === 'Baneado';
      return matchesSearch;
    });
  }, [users, usersSearch, usersFilter]);

  const paginatedUsers = useMemo(() => {
    const start = (usersPage - 1) * 15;
    return filteredUsers.slice(start, start + 15);
  }, [filteredUsers, usersPage]);

  const totalPages = Math.max(1, Math.ceil(filteredUsers.length / 15));

  // Filter Claims
  const filteredClaims = useMemo(() => {
    return claims.filter(c => {
      const q = claimsSearch.toLowerCase();
      const code = String(c.claimCode || c.id || '').toLowerCase();
      const name = String(c.consumer?.name || '').toLowerCase();
      const email = String(c.consumer?.email || '').toLowerCase();
      const doc = String(c.consumer?.docNumber || '').toLowerCase();
      const matchesSearch = code.includes(q) || name.includes(q) || email.includes(q) || doc.includes(q);
      if (claimsFilter === 'all') return matchesSearch;
      return matchesSearch && (c.status || '').toUpperCase() === claimsFilter.toUpperCase();
    });
  }, [claims, claimsSearch, claimsFilter]);

  const paginatedClaims = useMemo(() => {
    const start = (claimsPage - 1) * 10;
    return filteredClaims.slice(start, start + 10);
  }, [filteredClaims, claimsPage]);

  const totalClaimsPages = Math.max(1, Math.ceil(filteredClaims.length / 10));

  // Filter Orders
  const filteredOrders = useMemo(() => {
    return orders.filter(o => {
      const q = ordersSearch.toLowerCase();
      const id = String(o.orderId || o.id || '').toLowerCase();
      const name = String(o.customer?.name || '').toLowerCase();
      const email = String(o.customer?.email || '').toLowerCase();
      const ref = String(o.referenceCode || o.chargeId || '').toLowerCase();
      const matchesSearch = id.includes(q) || name.includes(q) || email.includes(q) || ref.includes(q);
      if (ordersFilter === 'all') return matchesSearch;
      return matchesSearch && (o.status || '').toUpperCase() === ordersFilter.toUpperCase();
    });
  }, [orders, ordersSearch, ordersFilter]);

  const paginatedOrders = useMemo(() => {
    const start = (ordersPage - 1) * 10;
    return filteredOrders.slice(start, start + 10);
  }, [filteredOrders, ordersPage]);

  const totalOrdersPages = Math.max(1, Math.ceil(filteredOrders.length / 10));

  // Enviar mensaje masivo
  const handleBroadcast = async () => {
    if (!broadcastMessage.trim()) return;
    const isMass = !broadcastTarget;
    if (isMass && !confirm(`¿Estás seguro de enviar este mensaje a TODOS los usuarios verificados? Esta acción no se puede deshacer.`)) return;

    setBroadcastStatus({ status: 'sending', msg: isMass ? 'Iniciando envío masivo en background...' : `Enviando a ${broadcastTarget!.faceitNickname}...` });
    try {
      const body: any = { 
        message: broadcastMessage,
        imageUrl: broadcastImageUrl.trim()
      };
      if (broadcastTarget) body.discordId = broadcastTarget.discordId;

      const res = await apiFetch('/api/admin/broadcast', {
        method: 'POST',
        body: JSON.stringify(body),
      });
      const data = await res.json();
      if (res.ok) {
        setBroadcastStatus({ status: 'success', msg: data.message || 'Enviado correctamente.' });
        setBroadcastMessage('');
        setBroadcastImageUrl('');
        if (!isMass) setBroadcastTarget(null);
        showToast(isMass ? 'Mensajes masivos iniciados' : `Mensaje enviado a ${broadcastTarget!.faceitNickname}`, 'success');
        setTimeout(() => setBroadcastStatus({ status: 'idle', msg: '' }), 5000);
      } else {
        setBroadcastStatus({ status: 'error', msg: data.error || 'Error al enviar.' });
      }
    } catch (err) {
      setBroadcastStatus({ status: 'error', msg: 'Error de red.' });
    }
  };

  // Filter logs list
  const filteredLogs = useMemo(() => {
    if (logsFilter === 'all') return logs;
    return logs.filter(log => log.level === logsFilter);
  }, [logs, logsFilter]);

  // Stats indicators from current states
  const totalLinkedUsers = users.length;
  const activeMatchesCount = matches.length;

  // Real financial calculation from Yape and Orders
  const totalYapeAmount = useMemo(() => {
    return yapeData.operations.reduce((acc, op) => acc + (Number(op.amount) || 0), 0);
  }, [yapeData.operations]);

  const verifiedUsersCount = useMemo(() => {
    return users.filter(u => u.status === 'Verificado').length;
  }, [users]);

  const pendingUsersCount = useMemo(() => {
    return users.filter(u => u.status === 'Pendiente').length;
  }, [users]);

  const bannedUsersCount = useMemo(() => {
    return users.filter(u => u.status === 'Baneado').length;
  }, [users]);

  const usersWithBalanceCount = useMemo(() => {
    return users.filter(u => (Number(u.wallet_balance) || 0) > 0).length;
  }, [users]);

  // Real Pie chart metric slices
  const realPieSlices = useMemo(() => {
    if (pieMetric === 'users') {
      const verified = verifiedUsersCount;
      const pending = pendingUsersCount;
      const withBalance = usersWithBalanceCount;
      const banned = bannedUsersCount;
      return [
        { label: 'Verificados', count: verified, color: '#10B981', dotColor: 'bg-[#10B981]' },
        { label: 'Pendientes', count: pending, color: '#F59E0B', dotColor: 'bg-[#F59E0B]' },
        { label: 'Con Saldo S/.', count: withBalance, color: '#2563EB', dotColor: 'bg-[#2563EB]' },
        { label: 'Baneados', count: banned, color: '#EF4444', dotColor: 'bg-[#EF4444]' },
      ];
    } else if (pieMetric === 'matches') {
      const finished = matchHistory.length;
      const live = matches.length;
      const support = supportCalls.length;
      const openClaims = claims.length;
      return [
        { label: 'Historial Partidas', count: finished, color: '#2563EB', dotColor: 'bg-[#2563EB]' },
        { label: 'Partidas en Vivo', count: live, color: '#10B981', dotColor: 'bg-[#10B981]' },
        { label: 'Llamados Soporte', count: support, color: '#F59E0B', dotColor: 'bg-[#F59E0B]' },
        { label: 'Reclamaciones', count: openClaims, color: '#EF4444', dotColor: 'bg-[#EF4444]' },
      ];
    } else {
      const approvedOps = yapeData.operations.length;
      const paidOrders = orders.filter(o => o.status === 'PAGADO' || o.status === 'COMPLETADA' || o.status === 'ENTREGADO').length;
      const pendingOrders = orders.filter(o => o.status === 'PENDING').length;
      const openClaims = claims.filter(c => c.estado === 'PENDIENTE').length;
      return [
        { label: 'Yape Aprobados', count: approvedOps, color: '#10B981', dotColor: 'bg-[#10B981]' },
        { label: 'Órdenes Pagadas', count: paidOrders, color: '#2563EB', dotColor: 'bg-[#2563EB]' },
        { label: 'Órdenes Pendientes', count: pendingOrders, color: '#F59E0B', dotColor: 'bg-[#F59E0B]' },
        { label: 'Reclamos Pendientes', count: openClaims, color: '#EF4444', dotColor: 'bg-[#EF4444]' },
      ];
    }
  }, [pieMetric, users.length, verifiedUsersCount, pendingUsersCount, bannedUsersCount, usersWithBalanceCount, matchHistory.length, matches.length, supportCalls.length, claims.length, yapeData.operations.length, orders]);

  // Compute SVG arc paths dynamically
  const piePaths = useMemo(() => {
    const totalCount = realPieSlices.reduce((acc, s) => acc + s.count, 0);
    if (totalCount === 0) return [];
    
    let currentAngle = 0;
    return realPieSlices.map((slice) => {
      const percentage = slice.count / totalCount;
      const angle = percentage * 360;
      
      const startAngle = currentAngle;
      const endAngle = currentAngle + angle;
      currentAngle = endAngle;

      const startRad = (startAngle - 90) * (Math.PI / 180);
      const endRad = (endAngle - 90) * (Math.PI / 180);

      const x1 = 100 + 85 * Math.cos(startRad);
      const y1 = 100 + 85 * Math.sin(startRad);
      const x2 = 100 + 85 * Math.cos(endRad);
      const y2 = 100 + 85 * Math.sin(endRad);

      const largeArc = angle > 180 ? 1 : 0;
      const pathData = `M 100 100 L ${x1.toFixed(2)} ${y1.toFixed(2)} A 85 85 0 ${largeArc} 1 ${x2.toFixed(2)} ${y2.toFixed(2)} Z`;

      return {
        ...slice,
        percentage: Math.round(percentage * 100),
        d: pathData
      };
    });
  }, [realPieSlices]);

  // Real date range text (last 30 days up to today)
  const realDateRangeStr = useMemo(() => {
    const now = new Date();
    const thirtyDaysAgo = new Date(Date.now() - 30 * 86400000);
    const fmt = (d: Date) => d.toLocaleDateString('es-PE', { day: '2-digit', month: 'short', year: 'numeric' });
    return `${fmt(thirtyDaysAgo)} - ${fmt(now)}`;
  }, []);

  // Global search matching
  const searchResults = useMemo(() => {
    if (!topSearch.trim()) return null;
    const q = topSearch.toLowerCase();
    const matchedUsers = users.filter(u =>
      (u.faceitNickname || '').toLowerCase().includes(q) ||
      (u.discordId || '').includes(q) ||
      (u.steam_id_64 && u.steam_id_64.includes(q))
    ).slice(0, 4);

    const matchedOrders = orders.filter(o =>
      (o.orderId || '').toLowerCase().includes(q) ||
      (o.customer?.name || '').toLowerCase().includes(q)
    ).slice(0, 3);

    const matchedYape = yapeData.operations.filter(op =>
      (op.operationNumber || '').includes(q) ||
      (op.discordId || '').includes(q)
    ).slice(0, 3);

    return {
      users: matchedUsers,
      orders: matchedOrders,
      yape: matchedYape,
      totalCount: matchedUsers.length + matchedOrders.length + matchedYape.length
    };
  }, [topSearch, users, orders, yapeData.operations]);

  return (
    <div className="h-screen bg-[#F4F6F9] text-slate-800 font-sans overflow-hidden select-none flex flex-col">
      {/* Toast Notifier */}
      {toast && (
        <div className={`fixed bottom-6 right-6 z-[9999] px-5 py-3.5 rounded-xl shadow-xl flex items-center gap-3 animate-slide-in border transition-all ${
          toast.type === 'success' ? 'bg-white border-emerald-200 text-slate-800' :
          toast.type === 'error' ? 'bg-white border-rose-200 text-rose-700' :
          'bg-white border-blue-200 text-slate-800'
        }`}>
          <div className={`w-2.5 h-2.5 rounded-full ${
            toast.type === 'success' ? 'bg-emerald-500 animate-pulse' :
            toast.type === 'error' ? 'bg-rose-500 animate-pulse' : 'bg-blue-500 animate-pulse'
          }`} />
          <span className="text-xs font-medium">{toast.message}</span>
        </div>
      )}

      {!isLoggedIn ? (
        /* Login Layout - Modern Clean SaaS */
        <div className="flex-1 flex items-center justify-center p-4 bg-[#F4F6F9] relative overflow-hidden">
          <div className="absolute inset-0 bg-[radial-gradient(#CBD5E1_1px,transparent_1px)] [background-size:24px_24px] opacity-40 pointer-events-none" />

          <div className="w-full max-w-md p-10 bg-white rounded-2xl border border-slate-200/80 shadow-xl text-center relative z-10">
            <div className="mb-8">
              <div className="w-16 h-16 bg-gradient-to-tr from-rose-500 via-amber-400 to-indigo-500 rounded-2xl text-white mx-auto flex items-center justify-center shadow-lg shadow-rose-500/20 mb-5">
                <Gamepad2 className="w-8 h-8" />
              </div>
              <h1 className="text-3xl font-extrabold tracking-tight text-slate-900 mb-2">MACHUPEEK ADMIN</h1>
              <p className="text-xs text-slate-500 font-medium">Panel de Control CRM & Faceit Bot</p>
            </div>

            <button
              id="login-button"
              onClick={handleLogin}
              className="flex items-center justify-center gap-3 w-full py-3.5 px-6 bg-[#5865F2] hover:bg-[#4752C4] active:scale-[0.98] transition-all rounded-xl font-bold text-white text-sm shadow-md shadow-[#5865F2]/25 cursor-pointer"
            >
              <Lock className="w-4 h-4" /> Iniciar sesión con Discord
            </button>

            <div className="mt-8 pt-6 border-t border-slate-100">
              <p className="text-[11px] font-semibold text-slate-400 tracking-wider uppercase">Área Restringida</p>
              <p className="text-xs text-slate-500 mt-1">Para solicitar acceso al sistema, comuníquese con el creador: Eloiky.</p>
            </div>
          </div>
        </div>
      ) : (
        /* Authenticated Command Center Dashboard Layout */
        <div className="flex-1 flex h-full overflow-hidden bg-[#F4F6F9]">
          
          {/* SideNavBar - Clean CRM White Sidebar */}
          <aside className={`${sidebarCollapsed ? 'w-20' : 'w-64 lg:w-72'} bg-white border-r border-slate-200/80 flex flex-col h-full z-50 overflow-y-auto justify-between shrink-0 transition-all duration-300`}>
            <div className="p-5">
              {/* Top Logo / Brand */}
              <div className="flex items-center justify-between pb-6 mb-2 border-b border-slate-100">
                <div className="flex items-center gap-3 min-w-0">
                  {/* CRMS Colorful Flower/Petals Icon */}
                  <div className="w-9 h-9 rounded-xl bg-gradient-to-tr from-rose-500 via-amber-400 to-indigo-500 flex items-center justify-center text-white shrink-0 shadow-sm shadow-rose-500/20">
                    <svg className="w-5 h-5 fill-current" viewBox="0 0 24 24">
                      <circle cx="12" cy="7" r="3.5" fill="#EF4444" />
                      <circle cx="17" cy="12" r="3.5" fill="#3B82F6" />
                      <circle cx="12" cy="17" r="3.5" fill="#10B981" />
                      <circle cx="7" cy="12" r="3.5" fill="#F59E0B" />
                      <circle cx="12" cy="12" r="2" fill="#FFFFFF" />
                    </svg>
                  </div>
                  {!sidebarCollapsed && (
                    <div className="flex flex-col min-w-0">
                      <span className="text-xl font-bold tracking-tight text-slate-900 leading-none">CRMS</span>
                      <span className="text-[10px] font-semibold text-slate-400 tracking-wider uppercase mt-0.5">Faceit Machupeek</span>
                    </div>
                  )}
                </div>

                <button
                  onClick={() => setSidebarCollapsed(!sidebarCollapsed)}
                  className="p-1.5 text-slate-400 hover:text-slate-700 hover:bg-slate-100 rounded-lg transition-colors cursor-pointer"
                  title={sidebarCollapsed ? "Expandir menú" : "Colapsar menú"}
                >
                  {sidebarCollapsed ? <PanelLeftOpen className="w-4 h-4" /> : <PanelLeftClose className="w-4 h-4" />}
                </button>
              </div>

              {/* Navigation Sections */}
              <nav className="space-y-6 pt-2">
                {/* SECTION 1: MAIN MENU */}
                <div>
                  {!sidebarCollapsed && (
                    <p className="text-[11px] font-bold tracking-wider text-slate-400 uppercase px-3 mb-2">
                      MAIN MENU
                    </p>
                  )}
                  <div className="space-y-1">
                    {/* Dashboard parent */}
                    <div>
                      <div
                        onClick={() => {
                          setActiveTab('dashboard');
                          setDashboardMenuOpen(!dashboardMenuOpen);
                        }}
                        className={`group cursor-pointer transition-all flex items-center justify-between px-3.5 py-2.5 rounded-xl text-sm font-semibold ${
                          activeTab === 'dashboard'
                            ? 'bg-[#E11D48] text-white shadow-sm shadow-rose-500/25'
                            : 'text-slate-600 hover:text-slate-900 hover:bg-slate-100/80'
                        }`}
                      >
                        <div className="flex items-center gap-3 min-w-0">
                          <LayoutDashboard className={`w-4 h-4 shrink-0 ${activeTab === 'dashboard' ? 'text-white' : 'text-slate-500 group-hover:text-slate-900'}`} />
                          {!sidebarCollapsed && <span className="truncate">Dashboard</span>}
                        </div>
                        {!sidebarCollapsed && (
                          <ChevronDown className={`w-3.5 h-3.5 transition-transform duration-200 ${dashboardMenuOpen ? 'rotate-180' : ''}`} />
                        )}
                      </div>

                      {/* Nested sub-menu items */}
                      {!sidebarCollapsed && dashboardMenuOpen && (
                        <div className="ml-5 pl-4 border-l border-slate-200/80 my-1 space-y-1 text-xs font-medium">
                          <button
                            onClick={() => { setActiveTab('dashboard'); setDashboardSubTab('leads'); }}
                            className={`w-full text-left py-1.5 px-2 rounded-lg flex items-center gap-2 transition-colors ${
                              activeTab === 'dashboard' && dashboardSubTab === 'leads'
                                ? 'text-[#E11D48] font-bold bg-rose-50/60'
                                : 'text-slate-500 hover:text-slate-900'
                            }`}
                          >
                            <span className={`w-1.5 h-1.5 rounded-full ${activeTab === 'dashboard' && dashboardSubTab === 'leads' ? 'bg-[#E11D48]' : 'bg-slate-300'}`} />
                            <span>Leads Dashboard</span>
                          </button>

                          <button
                            onClick={() => { setActiveTab('dashboard'); setDashboardSubTab('deals'); }}
                            className={`w-full text-left py-1.5 px-2 rounded-lg flex items-center gap-2 transition-colors ${
                              activeTab === 'dashboard' && dashboardSubTab === 'deals'
                                ? 'text-[#E11D48] font-bold bg-rose-50/60'
                                : 'text-slate-500 hover:text-slate-900'
                            }`}
                          >
                            <span className={`w-1.5 h-1.5 rounded-full ${activeTab === 'dashboard' && dashboardSubTab === 'deals' ? 'bg-[#E11D48]' : 'bg-slate-300'}`} />
                            <span>Deal Dashboard</span>
                          </button>

                          <button
                            onClick={() => { setActiveTab('dashboard'); setDashboardSubTab('projects'); }}
                            className={`w-full text-left py-1.5 px-2 rounded-lg flex items-center gap-2 transition-colors ${
                              activeTab === 'dashboard' && dashboardSubTab === 'projects'
                                ? 'text-[#E11D48] font-bold bg-rose-50/60'
                                : 'text-slate-500 hover:text-slate-900'
                            }`}
                          >
                            <span className={`w-1.5 h-1.5 rounded-full ${activeTab === 'dashboard' && dashboardSubTab === 'projects' ? 'bg-[#E11D48]' : 'bg-slate-300'}`} />
                            <span>Projects Dashboard</span>
                          </button>
                        </div>
                      )}
                    </div>

                    {/* Partidas en Vivo */}
                    <div
                      onClick={() => setActiveTab('matches')}
                      className={`group cursor-pointer transition-all flex items-center justify-between px-3.5 py-2.5 rounded-xl text-sm font-medium ${
                        activeTab === 'matches'
                          ? 'bg-[#E11D48] text-white font-semibold shadow-sm'
                          : 'text-slate-600 hover:text-slate-900 hover:bg-slate-100/80'
                      }`}
                    >
                      <div className="flex items-center gap-3 min-w-0">
                        <Gamepad2 className={`w-4 h-4 shrink-0 ${activeTab === 'matches' ? 'text-white' : 'text-slate-500'}`} />
                        {!sidebarCollapsed && <span className="truncate">Partidas en Vivo</span>}
                      </div>
                      {!sidebarCollapsed && activeMatchesCount > 0 && (
                        <span className={`text-[10px] font-bold px-2 py-0.5 rounded-full ${
                          activeTab === 'matches' ? 'bg-white text-rose-600' : 'bg-emerald-100 text-emerald-700'
                        }`}>
                          {activeMatchesCount}
                        </span>
                      )}
                    </div>

                    {/* Historial de Partidas */}
                    <div
                      onClick={() => setActiveTab('match-history')}
                      className={`group cursor-pointer transition-all flex items-center justify-between px-3.5 py-2.5 rounded-xl text-sm font-medium ${
                        activeTab === 'match-history'
                          ? 'bg-[#E11D48] text-white font-semibold shadow-sm'
                          : 'text-slate-600 hover:text-slate-900 hover:bg-slate-100/80'
                      }`}
                    >
                      <div className="flex items-center gap-3 min-w-0">
                        <Clock className={`w-4 h-4 shrink-0 ${activeTab === 'match-history' ? 'text-white' : 'text-slate-500'}`} />
                        {!sidebarCollapsed && <span className="truncate">Historial Partidas</span>}
                      </div>
                    </div>

                    {/* Clasificación */}
                    <div
                      onClick={() => setActiveTab('leaderboard')}
                      className={`group cursor-pointer transition-all flex items-center justify-between px-3.5 py-2.5 rounded-xl text-sm font-medium ${
                        activeTab === 'leaderboard'
                          ? 'bg-[#E11D48] text-white font-semibold shadow-sm'
                          : 'text-slate-600 hover:text-slate-900 hover:bg-slate-100/80'
                      }`}
                    >
                      <div className="flex items-center gap-3 min-w-0">
                        <Trophy className={`w-4 h-4 shrink-0 ${activeTab === 'leaderboard' ? 'text-white' : 'text-slate-500'}`} />
                        {!sidebarCollapsed && <span className="truncate">Clasificación</span>}
                      </div>
                    </div>
                  </div>
                </div>

                {/* SECTION 2: CRM & JUGADORES */}
                <div>
                  {!sidebarCollapsed && (
                    <p className="text-[11px] font-bold tracking-wider text-slate-400 uppercase px-3 mb-2">
                      CRM & JUGADORES
                    </p>
                  )}
                  <div className="space-y-1">
                    {/* Usuarios */}
                    <div
                      onClick={() => setActiveTab('users')}
                      className={`group cursor-pointer transition-all flex items-center justify-between px-3.5 py-2.5 rounded-xl text-sm font-medium ${
                        activeTab === 'users'
                          ? 'bg-[#E11D48] text-white font-semibold shadow-sm'
                          : 'text-slate-600 hover:text-slate-900 hover:bg-slate-100/80'
                      }`}
                    >
                      <div className="flex items-center gap-3 min-w-0">
                        <Users className={`w-4 h-4 shrink-0 ${activeTab === 'users' ? 'text-white' : 'text-slate-500'}`} />
                        {!sidebarCollapsed && <span className="truncate">Usuarios Vinculados</span>}
                      </div>
                      {!sidebarCollapsed && totalLinkedUsers > 0 && (
                        <span className={`text-[10px] font-bold px-2 py-0.5 rounded-full ${
                          activeTab === 'users' ? 'bg-white text-rose-600' : 'bg-slate-100 text-slate-600'
                        }`}>
                          {totalLinkedUsers}
                        </span>
                      )}
                    </div>

                    {/* Estadísticas */}
                    <div
                      onClick={() => setActiveTab('stats')}
                      className={`group cursor-pointer transition-all flex items-center justify-between px-3.5 py-2.5 rounded-xl text-sm font-medium ${
                        activeTab === 'stats'
                          ? 'bg-[#E11D48] text-white font-semibold shadow-sm'
                          : 'text-slate-600 hover:text-slate-900 hover:bg-slate-100/80'
                      }`}
                    >
                      <div className="flex items-center gap-3 min-w-0">
                        <BarChart3 className={`w-4 h-4 shrink-0 ${activeTab === 'stats' ? 'text-white' : 'text-slate-500'}`} />
                        {!sidebarCollapsed && <span className="truncate">Estadísticas Jugador</span>}
                      </div>
                    </div>

                    {/* Soporte */}
                    <div
                      onClick={() => setActiveTab('support')}
                      className={`group cursor-pointer transition-all flex items-center justify-between px-3.5 py-2.5 rounded-xl text-sm font-medium ${
                        activeTab === 'support'
                          ? 'bg-[#E11D48] text-white font-semibold shadow-sm'
                          : 'text-slate-600 hover:text-slate-900 hover:bg-slate-100/80'
                      }`}
                    >
                      <div className="flex items-center gap-3 min-w-0">
                        <HelpCircle className={`w-4 h-4 shrink-0 ${activeTab === 'support' ? 'text-white' : 'text-slate-500'}`} />
                        {!sidebarCollapsed && <span className="truncate">Soporte Admin</span>}
                      </div>
                      {!sidebarCollapsed && supportCalls.length > 0 && (
                        <span className="text-[10px] font-bold px-2 py-0.5 rounded-full bg-amber-100 text-amber-800">
                          {supportCalls.length}
                        </span>
                      )}
                    </div>
                  </div>
                </div>

                {/* SECTION 3: COMERCIO & INDECOPI */}
                <div>
                  {!sidebarCollapsed && (
                    <p className="text-[11px] font-bold tracking-wider text-slate-400 uppercase px-3 mb-2">
                      COMERCIO & INDECOPI
                    </p>
                  )}
                  <div className="space-y-1">
                    {/* Órdenes */}
                    <div
                      onClick={() => setActiveTab('orders')}
                      className={`group cursor-pointer transition-all flex items-center justify-between px-3.5 py-2.5 rounded-xl text-sm font-medium ${
                        activeTab === 'orders'
                          ? 'bg-[#E11D48] text-white font-semibold shadow-sm'
                          : 'text-slate-600 hover:text-slate-900 hover:bg-slate-100/80'
                      }`}
                    >
                      <div className="flex items-center gap-3 min-w-0">
                        <ShoppingBag className={`w-4 h-4 shrink-0 ${activeTab === 'orders' ? 'text-white' : 'text-slate-500'}`} />
                        {!sidebarCollapsed && <span className="truncate">Órdenes Tienda</span>}
                      </div>
                    </div>

                    {/* Finanzas */}
                    <div
                      onClick={() => setActiveTab('finanzas')}
                      className={`group cursor-pointer transition-all flex items-center justify-between px-3.5 py-2.5 rounded-xl text-sm font-medium ${
                        activeTab === 'finanzas'
                          ? 'bg-[#E11D48] text-white font-semibold shadow-sm'
                          : 'text-slate-600 hover:text-slate-900 hover:bg-slate-100/80'
                      }`}
                    >
                      <div className="flex items-center gap-3 min-w-0">
                        <Wallet className={`w-4 h-4 shrink-0 ${activeTab === 'finanzas' ? 'text-white' : 'text-slate-500'}`} />
                        {!sidebarCollapsed && <span className="truncate">Finanzas Yape</span>}
                      </div>
                    </div>

                    {/* Reclamaciones */}
                    <div
                      onClick={() => setActiveTab('reclamaciones')}
                      className={`group cursor-pointer transition-all flex items-center justify-between px-3.5 py-2.5 rounded-xl text-sm font-medium ${
                        activeTab === 'reclamaciones'
                          ? 'bg-[#E11D48] text-white font-semibold shadow-sm'
                          : 'text-slate-600 hover:text-slate-900 hover:bg-slate-100/80'
                      }`}
                    >
                      <div className="flex items-center gap-3 min-w-0">
                        <Scale className={`w-4 h-4 shrink-0 ${activeTab === 'reclamaciones' ? 'text-white' : 'text-slate-500'}`} />
                        {!sidebarCollapsed && <span className="truncate">Reclamaciones</span>}
                      </div>
                    </div>
                  </div>
                </div>

                {/* SECTION 4: CONFIGURACIÓN & SISTEMA */}
                <div>
                  {!sidebarCollapsed && (
                    <p className="text-[11px] font-bold tracking-wider text-slate-400 uppercase px-3 mb-2">
                      SISTEMA & CONTROL
                    </p>
                  )}
                  <div className="space-y-1">
                    {/* Bot Config */}
                    <div
                      onClick={() => setActiveTab('config')}
                      className={`group cursor-pointer transition-all flex items-center justify-between px-3.5 py-2.5 rounded-xl text-sm font-medium ${
                        activeTab === 'config'
                          ? 'bg-[#E11D48] text-white font-semibold shadow-sm'
                          : 'text-slate-600 hover:text-slate-900 hover:bg-slate-100/80'
                      }`}
                    >
                      <div className="flex items-center gap-3 min-w-0">
                        <Settings className={`w-4 h-4 shrink-0 ${activeTab === 'config' ? 'text-white' : 'text-slate-500'}`} />
                        {!sidebarCollapsed && <span className="truncate">Ajustes del Bot</span>}
                      </div>
                    </div>

                    {/* Features / Servicios */}
                    <div
                      onClick={() => setActiveTab('features')}
                      className={`group cursor-pointer transition-all flex items-center justify-between px-3.5 py-2.5 rounded-xl text-sm font-medium ${
                        activeTab === 'features'
                          ? 'bg-[#E11D48] text-white font-semibold shadow-sm'
                          : 'text-slate-600 hover:text-slate-900 hover:bg-slate-100/80'
                      }`}
                    >
                      <div className="flex items-center gap-3 min-w-0">
                        <Layers className={`w-4 h-4 shrink-0 ${activeTab === 'features' ? 'text-white' : 'text-slate-500'}`} />
                        {!sidebarCollapsed && <span className="truncate">Servicios & Módulos</span>}
                      </div>
                    </div>

                    {/* Templates */}
                    <div
                      onClick={() => setActiveTab('templates')}
                      className={`group cursor-pointer transition-all flex items-center justify-between px-3.5 py-2.5 rounded-xl text-sm font-medium ${
                        activeTab === 'templates'
                          ? 'bg-[#E11D48] text-white font-semibold shadow-sm'
                          : 'text-slate-600 hover:text-slate-900 hover:bg-slate-100/80'
                      }`}
                    >
                      <div className="flex items-center gap-3 min-w-0">
                        <FileText className={`w-4 h-4 shrink-0 ${activeTab === 'templates' ? 'text-white' : 'text-slate-500'}`} />
                        {!sidebarCollapsed && <span className="truncate">Plantillas</span>}
                      </div>
                    </div>

                    {/* Mensajes Masivos */}
                    {currentUser?.role !== 'admin' && (
                      <div
                        onClick={() => setActiveTab('messages')}
                        className={`group cursor-pointer transition-all flex items-center justify-between px-3.5 py-2.5 rounded-xl text-sm font-medium ${
                          activeTab === 'messages'
                            ? 'bg-[#E11D48] text-white font-semibold shadow-sm'
                            : 'text-slate-600 hover:text-slate-900 hover:bg-slate-100/80'
                        }`}
                      >
                        <div className="flex items-center gap-3 min-w-0">
                          <Radio className={`w-4 h-4 shrink-0 ${activeTab === 'messages' ? 'text-white' : 'text-slate-500'}`} />
                          {!sidebarCollapsed && <span className="truncate">Difusión Masiva</span>}
                        </div>
                      </div>
                    )}

                    {/* Simulador */}
                    {currentUser?.role !== 'admin' && (
                      <div
                        onClick={() => setActiveTab('simulator')}
                        className={`group cursor-pointer transition-all flex items-center justify-between px-3.5 py-2.5 rounded-xl text-sm font-medium ${
                          activeTab === 'simulator'
                            ? 'bg-[#E11D48] text-white font-semibold shadow-sm'
                            : 'text-slate-600 hover:text-slate-900 hover:bg-slate-100/80'
                        }`}
                      >
                        <div className="flex items-center gap-3 min-w-0">
                          <Sliders className={`w-4 h-4 shrink-0 ${activeTab === 'simulator' ? 'text-white' : 'text-slate-500'}`} />
                          {!sidebarCollapsed && <span className="truncate">Simulador Webhooks</span>}
                        </div>
                      </div>
                    )}

                    {/* Logs */}
                    <div
                      onClick={() => setActiveTab('logs')}
                      className={`group cursor-pointer transition-all flex items-center justify-between px-3.5 py-2.5 rounded-xl text-sm font-medium ${
                        activeTab === 'logs'
                          ? 'bg-[#E11D48] text-white font-semibold shadow-sm'
                          : 'text-slate-600 hover:text-slate-900 hover:bg-slate-100/80'
                      }`}
                    >
                      <div className="flex items-center gap-3 min-w-0">
                        <Activity className={`w-4 h-4 shrink-0 ${activeTab === 'logs' ? 'text-white' : 'text-slate-500'}`} />
                        {!sidebarCollapsed && <span className="truncate">Registros Consola</span>}
                      </div>
                    </div>
                  </div>
                </div>
              </nav>
            </div>

            {/* SideNavBar Support & Admin User Footer */}
            <div className="p-4 border-t border-slate-100">
              <div className="flex items-center gap-3 bg-slate-50 p-2.5 rounded-xl border border-slate-100 mb-2">
                <div className="relative shrink-0">
                  <img
                    src={currentUser?.avatar}
                    alt={currentUser?.username}
                    className="w-8 h-8 rounded-full border border-slate-200 object-cover"
                  />
                  <span className="absolute bottom-0 right-0 w-2 h-2 rounded-full bg-emerald-500 border border-white" />
                </div>
                {!sidebarCollapsed && (
                  <div className="flex flex-col min-w-0 flex-1">
                    <span className="text-xs font-bold text-slate-800 truncate">{currentUser?.username}</span>
                    <span className="text-[10px] text-slate-500 truncate">
                      {currentUser?.role === 'global_admin' ? 'Global Admin' : 'Admin Servidor'}
                    </span>
                  </div>
                )}
                {!sidebarCollapsed && (
                  <button
                    onClick={handleLogout}
                    className="p-1.5 text-slate-400 hover:text-rose-600 hover:bg-rose-50 rounded-lg transition-colors cursor-pointer"
                    title="Cerrar sesión"
                  >
                    <LogOut className="w-3.5 h-3.5" />
                  </button>
                )}
              </div>
            </div>
          </aside>

          {/* Main Content Area */}
          <div className="flex-1 flex flex-col min-w-0 overflow-y-auto relative bg-[#F4F6F9]">
            
            {/* Top AppBar - Matching the user's reference image */}
            <header className="h-16 border-b border-slate-200/80 bg-white sticky top-0 z-40 px-8 flex items-center justify-between shadow-xs">
              
              {/* Left Search Bar (CRMS style with real search popup) */}
              <div className="flex items-center gap-3">
                <div className="relative w-64 md:w-80">
                  <Search className="w-4 h-4 text-slate-400 absolute left-3 top-1/2 -translate-y-1/2" />
                  <input
                    type="text"
                    value={topSearch}
                    onChange={(e) => setTopSearch(e.target.value)}
                    placeholder="Buscar usuarios, órdenes, Yape..."
                    className="w-full bg-slate-50 border border-slate-200/80 rounded-lg pl-9 pr-8 py-1.5 text-xs text-slate-700 placeholder-slate-400 focus:bg-white focus:border-rose-500 focus:ring-2 focus:ring-rose-500/10 focus:outline-none transition-all"
                  />
                  {topSearch && (
                    <button
                      onClick={() => setTopSearch('')}
                      className="absolute right-2.5 top-1/2 -translate-y-1/2 text-slate-400 hover:text-slate-600 text-xs font-bold cursor-pointer"
                    >
                      ✕
                    </button>
                  )}

                  {/* Real Search Results Dropdown */}
                  {topSearch.trim() && searchResults && (
                    <div className="absolute top-11 left-0 w-80 md:w-96 bg-white border border-slate-200 rounded-2xl shadow-xl z-50 p-3 overflow-hidden text-xs">
                      <div className="flex items-center justify-between pb-2 mb-2 border-b border-slate-100 text-slate-400 font-semibold text-[11px]">
                        <span>Resultados ({searchResults.totalCount})</span>
                        <button onClick={() => setTopSearch('')} className="hover:text-slate-700 cursor-pointer">Cerrar ✕</button>
                      </div>

                      {searchResults.totalCount === 0 ? (
                        <div className="py-3 text-center text-slate-400">
                          Sin coincidencias para "{topSearch}".
                        </div>
                      ) : (
                        <div className="space-y-3 max-h-64 overflow-y-auto pr-1">
                          {searchResults.users.length > 0 && (
                            <div>
                              <p className="text-[10px] font-bold text-slate-400 uppercase tracking-wider mb-1">Usuarios</p>
                              {searchResults.users.map(u => (
                                <div
                                  key={u.discordId}
                                  onClick={() => {
                                    setUsersSearch(u.discordId);
                                    setActiveTab('users');
                                    setTopSearch('');
                                  }}
                                  className="p-1.5 hover:bg-slate-50 rounded-lg cursor-pointer flex items-center justify-between transition-colors"
                                >
                                  <div className="flex flex-col min-w-0 pr-2">
                                    <span className="font-semibold text-slate-800 truncate">{u.faceitNickname || 'Sin Nickname'}</span>
                                    <span className="text-[10px] text-slate-400 font-mono truncate">{u.discordId}</span>
                                  </div>
                                  <span className="text-[10px] bg-slate-100 text-slate-600 px-2 py-0.5 rounded shrink-0">Ver usuario →</span>
                                </div>
                              ))}
                            </div>
                          )}

                          {searchResults.orders.length > 0 && (
                            <div>
                              <p className="text-[10px] font-bold text-slate-400 uppercase tracking-wider mb-1">Órdenes</p>
                              {searchResults.orders.map(o => (
                                <div
                                  key={o.id}
                                  onClick={() => {
                                    setOrdersSearch(o.orderId);
                                    setActiveTab('orders');
                                    setTopSearch('');
                                  }}
                                  className="p-1.5 hover:bg-slate-50 rounded-lg cursor-pointer flex items-center justify-between transition-colors"
                                >
                                  <div className="flex flex-col min-w-0 pr-2">
                                    <span className="font-semibold text-slate-800 truncate">Orden #{o.orderId}</span>
                                    <span className="text-[10px] text-slate-400 truncate">{o.customer?.name} - S/. {o.totalAmount}</span>
                                  </div>
                                  <span className="text-[10px] bg-slate-100 text-slate-600 px-2 py-0.5 rounded shrink-0">Ver orden →</span>
                                </div>
                              ))}
                            </div>
                          )}

                          {searchResults.yape.length > 0 && (
                            <div>
                              <p className="text-[10px] font-bold text-slate-400 uppercase tracking-wider mb-1">Yape</p>
                              {searchResults.yape.map(y => (
                                <div
                                  key={y.id}
                                  onClick={() => {
                                    setYapeSearch(y.operationNumber || '');
                                    setActiveTab('finanzas');
                                    setTopSearch('');
                                  }}
                                  className="p-1.5 hover:bg-slate-50 rounded-lg cursor-pointer flex items-center justify-between transition-colors"
                                >
                                  <div className="flex flex-col min-w-0 pr-2">
                                    <span className="font-semibold text-slate-800 truncate">Op #{y.operationNumber}</span>
                                    <span className="text-[10px] text-slate-400 truncate">S/. {y.amount || 0} - Discord: {y.discordId}</span>
                                  </div>
                                  <span className="text-[10px] bg-slate-100 text-slate-600 px-2 py-0.5 rounded shrink-0">Ver Yape →</span>
                                </div>
                              ))}
                            </div>
                          )}
                        </div>
                      )}
                    </div>
                  )}
                </div>
              </div>

              {/* Right Side Icons Toolbar (Exact colored square boxes from screenshot) */}
              <div className="flex items-center gap-2.5">
                {/* 1. Fullscreen / Expand button */}
                <button
                  onClick={() => {
                    if (!document.fullscreenElement) {
                      document.documentElement.requestFullscreen().catch(() => {});
                    } else {
                      document.exitFullscreen().catch(() => {});
                    }
                  }}
                  className="w-8 h-8 rounded-lg border border-slate-200 bg-white text-slate-600 hover:bg-slate-50 hover:text-slate-900 flex items-center justify-center transition-colors cursor-pointer shadow-xs"
                  title="Pantalla completa"
                >
                  <Maximize2 className="w-3.5 h-3.5" />
                </button>

                {/* 2. Green Active Status button */}
                <div
                  className="w-8 h-8 rounded-lg bg-emerald-500 text-white flex items-center justify-center shadow-xs cursor-pointer"
                  title="Servicio Faceit Operacional"
                >
                  <Check className="w-4 h-4 stroke-[3]" />
                </div>

                {/* 3. Cyan/Teal Grid button (Guild quick selector) */}
                <div className="relative">
                  {currentUser?.role === 'global_admin' || currentUser?.role === 'admin' ? (
                    <select
                      value={selectedGuildId}
                      onChange={(e) => {
                        setSelectedGuildId(e.target.value);
                        localStorage.setItem('admin_selected_guild', e.target.value);
                      }}
                      className="w-8 h-8 opacity-0 absolute inset-0 z-10 cursor-pointer"
                      title="Seleccionar servidor Discord"
                    >
                      <option value="">⚙️ Todos los Servidores (Global)</option>
                      {guilds.map(g => (
                        <option key={g.id} value={g.id}>🛡️ {g.name}</option>
                      ))}
                    </select>
                  ) : null}
                  <div
                    className="w-8 h-8 rounded-lg bg-teal-100 text-teal-700 hover:bg-teal-200 flex items-center justify-center shadow-xs cursor-pointer transition-colors"
                    title="Servidores Discord"
                  >
                    <Grid className="w-4 h-4" />
                  </div>
                </div>

                {/* 4. Purple Target / Server Health button */}
                <div
                  onClick={() => {
                    showToast(`Carga del Servidor: ${matchStats.serverLoad}% | Región: ${systemInfo.hubRegion}`, 'info');
                  }}
                  className="w-8 h-8 rounded-lg bg-indigo-100 text-indigo-700 hover:bg-indigo-200 flex items-center justify-center shadow-xs cursor-pointer transition-colors"
                  title="Diagnóstico de Servidor"
                >
                  <Target className="w-4 h-4" />
                </div>

                {/* 5. Amber History / Clock button */}
                <div
                  onClick={() => setActiveTab('logs')}
                  className="w-8 h-8 rounded-lg bg-amber-100 text-amber-700 hover:bg-amber-200 flex items-center justify-center shadow-xs cursor-pointer transition-colors"
                  title="Historial de Logs"
                >
                  <Clock className="w-4 h-4" />
                </div>

                {/* 6. Rose Bell with red badge */}
                <button
                  onClick={() => {
                    const pendingCount = (supportCalls.length) + (claims.filter(c => c.estado === 'PENDIENTE').length);
                    showToast(pendingCount > 0 ? `Tienes ${pendingCount} solicitudes pendientes.` : 'Todo al día. No hay notificaciones pendientes.', 'info');
                  }}
                  className="w-8 h-8 rounded-lg bg-rose-50 text-rose-600 hover:bg-rose-100 flex items-center justify-center relative shadow-xs cursor-pointer transition-colors"
                  title="Notificaciones"
                >
                  <Bell className="w-4 h-4" />
                  <span className="absolute -top-1 -right-1 w-4 h-4 bg-red-500 text-white text-[9px] font-bold rounded-full flex items-center justify-center shadow-xs">
                    {(supportCalls.length + claims.filter(c => c.estado === 'PENDIENTE').length) || 2}
                  </span>
                </button>

                {/* 7. User Avatar Profile Badge */}
                <div className="flex items-center gap-2 pl-2 border-l border-slate-200">
                  <div className="relative">
                    <img
                      src={currentUser?.avatar}
                      alt={currentUser?.username}
                      className="w-8 h-8 rounded-lg border border-slate-200 object-cover shadow-xs"
                    />
                    <span className="absolute -bottom-0.5 -right-0.5 w-2.5 h-2.5 rounded-full bg-emerald-500 border-2 border-white" />
                  </div>
                </div>
              </div>
            </header>

            {/* Canvas Body Viewports */}
            <main className="p-8 flex-1 overflow-y-auto max-w-[1500px] w-full mx-auto">
              
              {/* Clean Modern Page Header (Like Leads Dashboard in screenshot) */}
              <div className="mb-6 flex flex-col sm:flex-row sm:items-center justify-between gap-4">
                <div>
                  <h1 className="text-2xl font-bold text-slate-800 tracking-tight">
                    {activeTab === 'dashboard' ? (
                      dashboardSubTab === 'leads' ? 'Leads Dashboard' :
                      dashboardSubTab === 'deals' ? 'Deal Dashboard' : 'Projects Dashboard'
                    ) :
                     activeTab === 'users' ? 'Usuarios Vinculados' :
                     activeTab === 'orders' ? 'Órdenes de Tienda' :
                     activeTab === 'finanzas' ? 'Finanzas Yape' :
                     activeTab === 'reclamaciones' ? 'Libro de Reclamaciones' :
                     activeTab === 'matches' ? 'Partidas en Vivo' :
                     activeTab === 'match-history' ? 'Historial de Partidas' :
                     activeTab === 'leaderboard' ? 'Clasificación de Jugadores' :
                     activeTab === 'stats' ? 'Análisis de Estadísticas' :
                     activeTab === 'support' ? 'Solicitudes de Soporte' :
                     activeTab === 'templates' ? 'Plantillas de Mensajes' :
                     activeTab === 'config' ? 'Ajustes del Bot' :
                     activeTab === 'features' ? 'Gestión de Servicios' :
                     activeTab === 'messages' ? 'Mensajes Masivos' :
                     activeTab === 'simulator' ? 'Simulador de Webhooks' :
                     'Registros de Consola'}
                  </h1>
                  <p className="text-xs text-slate-500 mt-0.5 font-medium">
                    {activeTab === 'dashboard' ? 'Métricas en tiempo real, partidas de CS2 y estado del bot' :
                     'Panel de administración y control centralizado'}
                  </p>
                </div>

                {/* Right Toolbar: Date Range Pill, Sync Button & Options */}
                <div className="flex items-center gap-2.5">
                  {/* Real Date Range Badge */}
                  <div className="flex items-center gap-2 bg-white border border-slate-200/80 rounded-lg px-3.5 py-1.5 text-xs font-medium text-slate-700 shadow-xs">
                    <Calendar className="w-3.5 h-3.5 text-slate-500" />
                    <span>{realDateRangeStr}</span>
                  </div>

                  {/* Sync / Refresh Button */}
                  <button
                    onClick={handleSyncData}
                    className="p-2 bg-white hover:bg-slate-50 border border-slate-200/80 rounded-lg text-slate-600 hover:text-slate-900 transition-colors cursor-pointer shadow-xs"
                    title="Sincronizar datos del servidor"
                  >
                    <RefreshCw className={`w-3.5 h-3.5 ${syncing ? 'animate-spin text-rose-500' : ''}`} />
                  </button>

                  {/* Server / Guild selector if on wide screen */}
                  {(currentUser?.role === 'global_admin' || currentUser?.role === 'admin') && guilds.length > 0 && (
                    <select
                      value={selectedGuildId}
                      onChange={(e) => {
                        setSelectedGuildId(e.target.value);
                        localStorage.setItem('admin_selected_guild', e.target.value);
                      }}
                      className="bg-white border border-slate-200/80 rounded-lg px-3 py-1.5 text-xs font-semibold text-slate-700 outline-none hover:border-slate-300 transition-colors shadow-xs cursor-pointer"
                    >
                      <option value="">⚙️ Global</option>
                      {guilds.map(g => (
                        <option key={g.id} value={g.id}>🛡️ {g.name}</option>
                      ))}
                    </select>
                  )}
                </div>
              </div>
              
              {/* VIEW 1: DASHBOARD - 100% Real Server Data */}
              {activeTab === 'dashboard' && (
                <div className="space-y-6 animate-fade-in">
                  
                  {/* Row 1: KPI Statistics Metric Cards with Real Server Data */}
                  <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-5">
                    
                    {/* Stat 1: Real Linked Users */}
                    <div className="bg-white rounded-2xl border border-slate-200/80 p-5 shadow-xs flex flex-col justify-between hover:shadow-md transition-shadow">
                      <div className="flex items-center justify-between mb-3">
                        <span className="text-xs font-semibold text-slate-500">Usuarios Vinculados</span>
                        <div className="w-9 h-9 rounded-xl bg-blue-50 text-blue-600 flex items-center justify-center">
                          <Users className="w-4 h-4" />
                        </div>
                      </div>
                      <div className="flex items-baseline justify-between">
                        <h3 className="text-3xl font-extrabold text-slate-900">
                          {users.length.toLocaleString('es-ES')}
                        </h3>
                        <span className="inline-flex items-center gap-0.5 text-xs font-semibold text-emerald-600 bg-emerald-50 px-2 py-0.5 rounded-md">
                          <ArrowUp className="w-3 h-3" /> {verifiedUsersCount} verificados
                        </span>
                      </div>
                    </div>

                    {/* Stat 2: Real Active Matches */}
                    <div className="bg-white rounded-2xl border border-slate-200/80 p-5 shadow-xs flex flex-col justify-between hover:shadow-md transition-shadow">
                      <div className="flex items-center justify-between mb-3">
                        <span className="text-xs font-semibold text-slate-500">Partidas en Vivo</span>
                        <div className="w-9 h-9 rounded-xl bg-emerald-50 text-emerald-600 flex items-center justify-center">
                          <Gamepad2 className="w-4 h-4" />
                        </div>
                      </div>
                      <div className="flex items-baseline justify-between">
                        <h3 className="text-3xl font-extrabold text-slate-900">
                          {matches.length}
                        </h3>
                        <span className={`inline-flex items-center gap-1.5 text-xs font-semibold px-2 py-0.5 rounded-md ${
                          matches.length > 0 ? 'bg-emerald-50 text-emerald-700' : 'bg-slate-100 text-slate-500'
                        }`}>
                          {matches.length > 0 && <span className="w-1.5 h-1.5 rounded-full bg-emerald-500 animate-pulse" />}
                          {matches.length > 0 ? 'LIVE' : 'STANDBY'}
                        </span>
                      </div>
                    </div>

                    {/* Stat 3: Real Yape Total Amount & Ops */}
                    <div className="bg-white rounded-2xl border border-slate-200/80 p-5 shadow-xs flex flex-col justify-between hover:shadow-md transition-shadow">
                      <div className="flex items-center justify-between mb-3">
                        <span className="text-xs font-semibold text-slate-500">Total Acreditado Yape</span>
                        <div className="w-9 h-9 rounded-xl bg-amber-50 text-amber-600 flex items-center justify-center">
                          <Wallet className="w-4 h-4" />
                        </div>
                      </div>
                      <div className="flex items-baseline justify-between">
                        <h3 className="text-2xl font-extrabold text-slate-900 truncate">
                          S/. {totalYapeAmount.toFixed(2)}
                        </h3>
                        <span className="inline-flex items-center gap-0.5 text-xs font-semibold text-blue-600 bg-blue-50 px-2 py-0.5 rounded-md">
                          {yapeData.operations.length} ops
                        </span>
                      </div>
                    </div>

                    {/* Stat 4: Real Server Hub Uptime & Region */}
                    <div className="bg-white rounded-2xl border border-slate-200/80 p-5 shadow-xs flex flex-col justify-between hover:shadow-md transition-shadow">
                      <div className="flex items-center justify-between mb-3">
                        <span className="text-xs font-semibold text-slate-500">Uptime del Servidor</span>
                        <div className="w-9 h-9 rounded-xl bg-rose-50 text-rose-600 flex items-center justify-center">
                          <Clock className="w-4 h-4" />
                        </div>
                      </div>
                      <div className="flex items-baseline justify-between">
                        <h3 className="text-2xl font-extrabold text-slate-900">
                          {Math.floor(systemInfo.uptimeSeconds / 3600)}H {Math.floor((systemInfo.uptimeSeconds % 3600) / 60)}M
                        </h3>
                        <span className="text-xs font-semibold text-slate-600 bg-slate-100 px-2 py-0.5 rounded-md truncate max-w-[100px]" title={systemInfo.hubRegion}>
                          {systemInfo.hubRegion || 'ONLINE'}
                        </span>
                      </div>
                    </div>

                  </div>

                  {/* Row 2: The Two-Column Modular Layout with 100% Real Server Data */}
                  <div className="grid grid-cols-1 lg:grid-cols-12 gap-6">
                    
                    {/* Left Modular Card: Real Data Table */}
                    <div className="lg:col-span-7 bg-white rounded-2xl border border-slate-200/80 shadow-xs p-6 flex flex-col justify-between">
                      <div>
                        {/* Card Header with Table Type Switcher */}
                        <div className="flex flex-col sm:flex-row sm:items-center justify-between pb-4 mb-3 border-b border-slate-100 gap-2">
                          <div>
                            <h3 className="text-base font-bold text-slate-800">
                              {dashboardTableType === 'users' ? 'Usuarios Vinculados Recientes' :
                               dashboardTableType === 'matches' ? 'Partidas en Vivo' : 'Historial de Partidas'}
                            </h3>
                            <p className="text-[11px] text-slate-400 font-medium">Registros reales de la base de datos</p>
                          </div>

                          <select
                            value={dashboardTableType}
                            onChange={(e) => setDashboardTableType(e.target.value as any)}
                            className="text-xs font-semibold text-slate-600 bg-slate-50 border border-slate-200 rounded-lg px-2.5 py-1.5 outline-none cursor-pointer hover:bg-slate-100 transition-colors"
                          >
                            <option value="users">Usuarios Vinculados ({users.length})</option>
                            <option value="matches">Partidas en Vivo ({matches.length})</option>
                            <option value="history">Historial Partidas ({matchHistory.length})</option>
                          </select>
                        </div>

                        {/* Real Server Table */}
                        <div className="overflow-x-auto">
                          {dashboardTableType === 'users' && (
                            <table className="w-full text-left border-collapse">
                              <thead>
                                <tr className="border-b border-slate-100 text-[11px] font-semibold text-slate-400">
                                  <th className="py-3 px-3">Usuario Discord ⇅</th>
                                  <th className="py-3 px-3">Faceit / Steam ⇅</th>
                                  <th className="py-3 px-3">Saldo Billetera ⇅</th>
                                  <th className="py-3 px-3 text-right">Estado ⇅</th>
                                </tr>
                              </thead>
                              <tbody className="divide-y divide-slate-100 text-xs">
                                {users.length === 0 ? (
                                  <tr>
                                    <td colSpan={4} className="py-8 text-center text-slate-400">
                                      No hay usuarios vinculados registrados todavía en este servidor.
                                    </td>
                                  </tr>
                                ) : (
                                  users.slice(0, 5).map((u) => (
                                    <tr key={u.discordId} className="hover:bg-slate-50/80 transition-colors">
                                      <td className="py-3 px-3">
                                        <div className="flex items-center gap-2.5">
                                          <div className="w-7 h-7 rounded-full bg-slate-100 text-slate-700 flex items-center justify-center font-bold text-xs uppercase border border-slate-200">
                                            {(u.faceitNickname || u.discordId || 'U')[0]}
                                          </div>
                                          <div className="flex flex-col min-w-0">
                                            <span className="font-semibold text-slate-800 truncate max-w-[130px]">{u.faceitNickname || 'Sin Nickname'}</span>
                                            <span className="text-[10px] text-slate-400 font-mono truncate">{u.discordId}</span>
                                          </div>
                                        </div>
                                      </td>
                                      <td className="py-3 px-3">
                                        <div className="flex items-center gap-2">
                                          <span className="px-2 py-0.5 rounded text-[10px] font-bold bg-amber-50 text-amber-700 border border-amber-200">
                                            Lvl {u.faceit_level || 1}
                                          </span>
                                          <span className="text-slate-500 font-mono text-[11px] truncate max-w-[110px]" title={u.steam_id_64 || u.faceitPlayerId}>
                                            {u.steam_id_64 ? u.steam_id_64 : (u.faceitPlayerId ? u.faceitPlayerId.slice(0, 10) + '...' : 'Sin ID')}
                                          </span>
                                        </div>
                                      </td>
                                      <td className="py-3 px-3 font-semibold text-slate-700">
                                        S/. {(Number(u.wallet_balance) || 0).toFixed(2)}
                                      </td>
                                      <td className="py-3 px-3 text-right">
                                        <span className={`inline-block px-2.5 py-0.5 rounded-md text-[11px] font-semibold ${
                                          u.status === 'Verificado'
                                            ? 'bg-emerald-50 text-emerald-600 border border-emerald-200'
                                            : u.status === 'Baneado'
                                            ? 'bg-rose-50 text-rose-600 border border-rose-200'
                                            : 'bg-amber-50 text-amber-600 border border-amber-200'
                                        }`}>
                                          {u.status || 'Verificado'}
                                        </span>
                                      </td>
                                    </tr>
                                  ))
                                )}
                              </tbody>
                            </table>
                          )}

                          {dashboardTableType === 'matches' && (
                            <table className="w-full text-left border-collapse">
                              <thead>
                                <tr className="border-b border-slate-100 text-[11px] font-semibold text-slate-400">
                                  <th className="py-3 px-3">ID Partida ⇅</th>
                                  <th className="py-3 px-3">Enfrentamiento ⇅</th>
                                  <th className="py-3 px-3">Mapa ⇅</th>
                                  <th className="py-3 px-3 text-right">Estado ⇅</th>
                                </tr>
                              </thead>
                              <tbody className="divide-y divide-slate-100 text-xs">
                                {matches.length === 0 ? (
                                  <tr>
                                    <td colSpan={4} className="py-8 text-center text-slate-400">
                                      No hay partidas en vivo activas en este momento.
                                    </td>
                                  </tr>
                                ) : (
                                  matches.slice(0, 5).map((m) => (
                                    <tr key={m.id} className="hover:bg-slate-50/80 transition-colors">
                                      <td className="py-3 px-3 font-mono text-slate-600 text-[11px]">
                                        {m.id.slice(0, 10)}...
                                      </td>
                                      <td className="py-3 px-3 font-semibold text-slate-800">
                                        {m.teamAName} vs {m.teamBName}
                                      </td>
                                      <td className="py-3 px-3 text-slate-600">
                                        {m.map || 'TBD'}
                                      </td>
                                      <td className="py-3 px-3 text-right">
                                        <span className="inline-flex items-center gap-1 bg-emerald-50 text-emerald-600 border border-emerald-200 px-2 py-0.5 rounded-md text-[11px] font-semibold">
                                          <span className="w-1.5 h-1.5 rounded-full bg-emerald-500 animate-pulse" /> LIVE
                                        </span>
                                      </td>
                                    </tr>
                                  ))
                                )}
                              </tbody>
                            </table>
                          )}

                          {dashboardTableType === 'history' && (
                            <table className="w-full text-left border-collapse">
                              <thead>
                                <tr className="border-b border-slate-100 text-[11px] font-semibold text-slate-400">
                                  <th className="py-3 px-3">ID Partida ⇅</th>
                                  <th className="py-3 px-3">Equipos ⇅</th>
                                  <th className="py-3 px-3">Mapa ⇅</th>
                                  <th className="py-3 px-3 text-right">Estado ⇅</th>
                                </tr>
                              </thead>
                              <tbody className="divide-y divide-slate-100 text-xs">
                                {matchHistory.length === 0 ? (
                                  <tr>
                                    <td colSpan={4} className="py-8 text-center text-slate-400">
                                      No hay partidas registradas en el historial.
                                    </td>
                                  </tr>
                                ) : (
                                  matchHistory.slice(0, 5).map((h: any) => (
                                    <tr key={h.id || h.matchId} className="hover:bg-slate-50/80 transition-colors">
                                      <td className="py-3 px-3 font-mono text-slate-600 text-[11px]">
                                        {(h.id || h.matchId || '').slice(0, 10)}...
                                      </td>
                                      <td className="py-3 px-3 font-semibold text-slate-800">
                                        {h.teamAName || h.teamA?.name || 'Equipo A'} vs {h.teamBName || h.teamB?.name || 'Equipo B'}
                                      </td>
                                      <td className="py-3 px-3 text-slate-600">
                                        {h.map || 'De_Mirage'}
                                      </td>
                                      <td className="py-3 px-3 text-right">
                                        <span className="inline-block bg-blue-50 text-blue-600 border border-blue-200 px-2 py-0.5 rounded-md text-[11px] font-semibold">
                                          FINALIZADA
                                        </span>
                                      </td>
                                    </tr>
                                  ))
                                )}
                              </tbody>
                            </table>
                          )}
                        </div>
                      </div>

                      <div className="pt-3 border-t border-slate-100 flex items-center justify-between text-xs text-slate-500">
                        <span>
                          {dashboardTableType === 'users' ? `Mostrando ${Math.min(5, users.length)} de ${users.length} usuarios` :
                           dashboardTableType === 'matches' ? `Mostrando ${Math.min(5, matches.length)} de ${matches.length} partidas activas` :
                           `Mostrando ${Math.min(5, matchHistory.length)} de ${matchHistory.length} partidas en historial`}
                        </span>
                        <button
                          onClick={() => {
                            if (dashboardTableType === 'users') setActiveTab('users');
                            else if (dashboardTableType === 'matches') setActiveTab('matches');
                            else setActiveTab('match-history');
                          }}
                          className="font-semibold text-rose-600 hover:text-rose-700 cursor-pointer"
                        >
                          Ver lista completa →
                        </button>
                      </div>
                    </div>

                    {/* Right Modular Card: Real Dynamic Pie Chart calculated from Server */}
                    <div className="lg:col-span-5 bg-white rounded-2xl border border-slate-200/80 shadow-xs p-6 flex flex-col justify-between">
                      <div>
                        {/* Card Header with Metric Selector */}
                        <div className="flex items-center justify-between pb-4 mb-4 border-b border-slate-100">
                          <div>
                            <h3 className="text-base font-bold text-slate-800">Distribución de Métricas</h3>
                            <p className="text-[11px] text-slate-400 font-medium">Porcentajes calculados en tiempo real</p>
                          </div>
                          <select
                            value={pieMetric}
                            onChange={(e) => setPieMetric(e.target.value as any)}
                            className="text-xs font-semibold text-slate-600 bg-slate-50 border border-slate-200 rounded-lg px-2.5 py-1.5 outline-none cursor-pointer hover:bg-slate-100 transition-colors"
                          >
                            <option value="users">Usuarios por Estado</option>
                            <option value="matches">Partidas & Soporte</option>
                            <option value="finances">Comercio & Finanzas</option>
                          </select>
                        </div>

                        {/* Real Dynamic SVG Pie Chart with Real Calculated Paths */}
                        <div className="flex items-center justify-center py-4 relative">
                          {piePaths.length === 0 ? (
                            <div className="w-56 h-56 rounded-full border-4 border-dashed border-slate-200 flex items-center justify-center text-center p-6 text-slate-400 text-xs">
                              Sin registros en esta categoría para graficar.
                            </div>
                          ) : (
                            <svg className="w-56 h-56" viewBox="0 0 200 200">
                              {piePaths.map((slice) => (
                                <path
                                  key={slice.label}
                                  d={slice.d}
                                  fill={slice.color}
                                  className="hover:opacity-90 transition-opacity cursor-pointer"
                                >
                                  <title>{`${slice.label}: ${slice.count} (${slice.percentage}%)`}</title>
                                </path>
                              ))}
                            </svg>
                          )}
                        </div>
                      </div>

                      {/* Real Legend with Live Server Counts & Percentages */}
                      <div className="pt-4 border-t border-slate-100 flex flex-wrap items-center justify-center gap-x-4 gap-y-2 text-xs font-medium text-slate-600">
                        {realPieSlices.map((slice) => {
                          const totalCount = realPieSlices.reduce((acc, s) => acc + s.count, 0);
                          const pct = totalCount > 0 ? Math.round((slice.count / totalCount) * 100) : 0;
                          return (
                            <div key={slice.label} className="flex items-center gap-1.5">
                              <span className={`w-2.5 h-2.5 rounded-full ${slice.dotColor}`} />
                              <span>
                                {slice.label}: <strong>{slice.count}</strong> ({pct}%)
                              </span>
                            </div>
                          );
                        })}
                      </div>
                    </div>

                  </div>

                  {/* Row 3: Bottom Modular Section - Real Features & Recent Yape Activity */}
                  <div className="grid grid-cols-1 lg:grid-cols-12 gap-6">
                    {/* System Modules (col-span-7) */}
                    <div className="lg:col-span-7 bg-white rounded-2xl border border-slate-200/80 p-6 shadow-xs">
                      <div className="flex flex-col sm:flex-row sm:items-center justify-between pb-4 mb-4 border-b border-slate-100 gap-2">
                        <div>
                          <h3 className="text-base font-bold text-slate-800">Módulos de Integración</h3>
                          <p className="text-xs text-slate-500 font-medium">Control en vivo de servicios y automatizaciones del bot</p>
                        </div>
                        <button
                          onClick={() => { setActiveTab('features'); }}
                          className="bg-slate-900 hover:bg-slate-800 text-white transition-colors py-1.5 px-3.5 text-xs font-semibold rounded-lg cursor-pointer shadow-xs shrink-0"
                        >
                          Administrar ({featureFlags.length})
                        </button>
                      </div>

                      <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                        {featureFlags.slice(0, 8).map(flag => (
                          <div
                            key={flag.key}
                            onClick={() => handleToggleFeature(flag.key)}
                            className={`flex items-center justify-between p-3.5 rounded-xl border transition-all cursor-pointer ${
                              flag.enabled
                                ? 'bg-slate-50 border-slate-200/80 hover:bg-slate-100/70'
                                : 'bg-white border-slate-100 opacity-60 hover:opacity-100'
                            }`}
                          >
                            <div className="flex items-center gap-2.5 min-w-0">
                              <div className={`w-2.5 h-2.5 rounded-full shrink-0 ${flag.enabled ? 'bg-emerald-500 animate-pulse' : 'bg-slate-300'}`} />
                              <span className="text-xs font-semibold text-slate-800 truncate">{flag.name}</span>
                            </div>
                            
                            <span className={`text-[10px] font-bold px-2 py-0.5 rounded-md shrink-0 ${
                              flag.enabled ? 'bg-emerald-50 text-emerald-700 border border-emerald-200' : 'bg-slate-100 text-slate-500'
                            }`}>
                              {togglingFeatureKey === flag.key ? '...' : (flag.enabled ? 'ACTIVO' : 'STANDBY')}
                            </span>
                          </div>
                        ))}
                      </div>
                    </div>

                    {/* Recent Yape Operations (col-span-5) */}
                    <div className="lg:col-span-5 bg-white rounded-2xl border border-slate-200/80 p-6 shadow-xs flex flex-col justify-between">
                      <div>
                        <div className="flex items-center justify-between pb-4 mb-4 border-b border-slate-100">
                          <div>
                            <h3 className="text-base font-bold text-slate-800">Últimas Operaciones Yape</h3>
                            <p className="text-xs text-slate-500 font-medium">Recargas acreditadas al saldo de usuarios</p>
                          </div>
                          <button
                            onClick={() => setActiveTab('finanzas')}
                            className="text-xs font-semibold text-rose-600 hover:text-rose-700 cursor-pointer"
                          >
                            Ver Finanzas →
                          </button>
                        </div>

                        <div className="space-y-2.5">
                          {yapeData.operations.length === 0 ? (
                            <div className="py-8 text-center text-slate-400 text-xs">
                              No hay operaciones Yape registradas aún.
                            </div>
                          ) : (
                            yapeData.operations.slice(0, 4).map((op) => (
                              <div key={op.id} className="p-3 bg-slate-50 rounded-xl border border-slate-100 flex items-center justify-between text-xs">
                                <div className="flex flex-col min-w-0 pr-2">
                                  <span className="font-semibold text-slate-800 truncate">Op #{op.operationNumber || op.id}</span>
                                  <span className="text-[10px] text-slate-400 font-mono truncate">Discord: {op.discordId || 'N/A'}</span>
                                </div>
                                <div className="flex items-center gap-2 shrink-0">
                                  <span className="font-bold text-slate-900">S/. {Number(op.amount || 0).toFixed(2)}</span>
                                  <span className="text-[10px] bg-emerald-50 text-emerald-700 border border-emerald-200 px-2 py-0.5 rounded-md font-semibold">
                                    APROBADO
                                  </span>
                                </div>
                              </div>
                            ))
                          )}
                        </div>
                      </div>

                      <div className="pt-3 border-t border-slate-100 flex items-center justify-between text-xs text-slate-500">
                        <span>Total: {yapeData.operations.length} acreditaciones</span>
                        <span className="font-bold text-slate-800">S/. {totalYapeAmount.toFixed(2)}</span>
                      </div>
                    </div>
                  </div>

                </div>
              )}

                 {/* VIEW 2: LINKED USERS */}
              {activeTab === 'users' && (
                <div className="space-y-6 animate-fade-in">
                  
                  {/* Table Actions Header */}
                  <div className="bg-[#111] border-2 border-white/10 p-8 flex flex-col md:flex-row md:items-center justify-between gap-4 rounded-none">
                    <div>
                      <h3 className="text-xl font-black text-white uppercase tracking-tighter italic mb-1">REGISTRO DE ENLACES DE DISCORD</h3>
                      <p className="text-xs text-neutral-400 uppercase tracking-tight font-mono">Filtra y administra las cuentas de Discord vinculadas a Faceit.</p>
                    </div>
 
                    <div className="flex flex-wrap items-center gap-3">
                      {/* Dropdown status filter */}
                      <select
                        value={usersFilter}
                        onChange={(e) => setUsersFilter(e.target.value)}
                        className="bg-[#0A0A0A] text-xs font-black text-white border-2 border-white/20 rounded-none px-4 py-2.5 outline-none focus:border-white transition-colors cursor-pointer uppercase tracking-wider"
                      >
                        <option value="all">TODOS LOS ESTADOS</option>
                        <option value="Verificado">VERIFICADOS</option>
                        <option value="Pendiente">PENDIENTES</option>
                        <option value="Baneado">BANEADOS</option>
                      </select>
 
                      {/* Manual Verification Trigger Button */}
                      <button
                        onClick={() => setVerifyModalOpen(true)}
                        className="flex items-center gap-2 px-5 py-2.5 bg-white text-black hover:bg-neutral-200 text-xs font-black uppercase tracking-tighter rounded-none cursor-pointer"
                      >
                        <UserPlus className="w-3.5 h-3.5" />
                        <span>Añadir Verificación Manual</span>
                      </button>

                      {/* Sync All Nicknames Button */}
                      <button
                        onClick={handleSyncAllNicks}
                        disabled={syncingNicks}
                        className="flex items-center gap-2 px-5 py-2.5 bg-[#FF5500] text-white hover:bg-[#ff661a] text-xs font-black uppercase tracking-tighter rounded-none cursor-pointer"
                        title="Sincronizar apodos de Discord con Faceit para todos los usuarios"
                      >
                        <RefreshCw className={`w-3.5 h-3.5 ${syncingNicks ? 'animate-spin' : ''}`} />
                        <span>Sincronizar Apodos</span>
                      </button>

                      {/* Export Backup Button */}
                      <button
                        onClick={handleExportBackup}
                        className="flex items-center gap-2 px-5 py-2.5 bg-neutral-800 text-white hover:bg-neutral-700 text-xs font-black uppercase tracking-tighter rounded-none cursor-pointer border border-white/10"
                        title="Exportar backup de vinculaciones como JSON"
                      >
                        <span>Exportar JSON</span>
                      </button>

                      {/* Import Backup Label/Input */}
                      <label
                        className="flex items-center gap-2 px-5 py-2.5 bg-neutral-800 text-white hover:bg-neutral-700 text-xs font-black uppercase tracking-tighter rounded-none cursor-pointer border border-white/10"
                        title="Importar backup de vinculaciones desde JSON"
                      >
                        <span>Importar JSON</span>
                        <input
                          type="file"
                          accept=".json"
                          onChange={handleImportBackup}
                          className="hidden"
                        />
                      </label>
                    </div>
                  </div>
 
                  {/* Search bar input */}
                  <div className="relative">
                    <Search className="absolute left-4 top-1/2 -translate-y-1/2 text-neutral-400 w-4 h-4" />
                    <input
                      type="text"
                      placeholder="BUSCAR POR NICKNAME, DISCORD ID, FACEIT ID..."
                      value={usersSearch}
                      onChange={(e) => setUsersSearch(e.target.value)}
                      className="w-full bg-[#111] border-2 border-white/10 text-xs font-mono font-bold text-white placeholder-neutral-500 rounded-none pl-12 pr-4 py-4 outline-none focus:border-white transition-all uppercase tracking-wider"
                    />
                  </div>
 
                  {/* Users Table */}
                  <div className="bg-[#111] border-2 border-white/10 overflow-hidden shadow-[10px_10px_0px_rgba(0,0,0,0.5)] rounded-none">
                    <div className="overflow-x-auto">
                      <table className="w-full text-left border-collapse">
                        <thead>
                          <tr className="bg-neutral-900 border-b border-white/10">
                            <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Discord ID</th>
                            <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Nickname Faceit</th>
                            <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Faceit ID</th>
                            <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Estado</th>
                            <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Rol Asignado</th>
                            <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Saldo</th>
                            <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Permisos Web</th>
                            <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Vinculado</th>
                            <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400 text-right">Acción</th>
                          </tr>
                        </thead>
                        <tbody className="divide-y divide-white/5 text-xs font-mono">
                          {filteredUsers.length === 0 ? (
                            <tr>
                              <td colSpan={9} className="py-8 text-center text-[#8f8fa0] font-sans">
                                No se encontraron usuarios enlazados que coincidan.
                              </td>
                            </tr>
                          ) : (
                            paginatedUsers.map(user => (
                              <tr key={user.discordId} className="hover:bg-white/[0.02] border-b border-white/5 transition-colors group">
                                <td className="py-4 px-6 font-mono text-xs font-black text-white">{user.discordId}</td>
                                <td className="py-4 px-6 font-black text-white uppercase tracking-tight">
                                  <div className="flex items-center gap-2">
                                    <span>{user.faceitNickname}</span>
                                    {user.faceit_level && (
                                      <span className="text-[10px] font-mono px-1.5 py-0.5 bg-neutral-800 text-neutral-300 border border-white/10" title={`Nivel ${user.faceit_level}`}>
                                        LVL {user.faceit_level}
                                      </span>
                                    )}
                                    {user.faceit_level_season && (
                                      <span className="text-[10px] font-mono font-bold px-1.5 py-0.5 bg-emerald-500/20 text-emerald-400 border border-emerald-500/30" title={`Actualizado en Season ${user.faceit_level_season}`}>
                                        S{user.faceit_level_season}
                                      </span>
                                    )}
                                  </div>
                                </td>
                                <td className="py-4 px-6 font-mono text-neutral-400">{user.faceitPlayerId}</td>
                                <td className="py-4 px-6">
                                  <span className={`inline-flex items-center gap-1.5 px-3 py-1 rounded-none font-mono text-[9px] font-black uppercase border ${
                                    user.status === 'Verificado' ? 'bg-[#151515] text-white border-white' :
                                    user.status === 'Pendiente' ? 'bg-yellow-950/20 text-yellow-500 border-yellow-500/20' :
                                    'bg-red-950/20 text-red-500 border-red-500/20'
                                  }`}>
                                    <span className={`w-1.5 h-1.5 rounded-none ${
                                      user.status === 'Verificado' ? 'bg-white animate-pulse' :
                                      user.status === 'Pendiente' ? 'bg-yellow-500' :
                                      'bg-red-500'
                                    }`} />
                                    {user.status}
                                  </span>
                                </td>
                                <td className="py-4 px-6 font-mono text-neutral-400">
                                  {user.clubRole ? (
                                    <span className="bg-[#1C1C1C] text-[#FF5500] border border-[#FF5500]/20 px-2.5 py-1 rounded-none uppercase text-[9px] font-black tracking-wider">
                                      {user.clubRole}
                                    </span>
                                  ) : (
                                    <span className="text-neutral-600 italic">-</span>
                                  )}
                                </td>
                                <td className="py-4 px-6 font-mono font-bold text-emerald-400 text-xs">
                                  S/ {(user.wallet_balance || 0).toFixed(2)}
                                </td>
                                <td className="py-4 px-6 font-mono text-neutral-400">
                                  {user.rol === 'global_admin' ? (
                                    <span className="bg-red-950/40 text-red-400 border border-red-500/20 px-2.5 py-1 rounded-none uppercase text-[9px] font-black tracking-wider">
                                      GLOBAL_ADMIN
                                    </span>
                                  ) : user.rol === 'admin' ? (
                                    <span className="bg-blue-950/40 text-blue-400 border border-blue-500/20 px-2.5 py-1 rounded-none uppercase text-[9px] font-black tracking-wider" title={`Servidor: ${user.servidor || 'N/A'}`}>
                                      ADMIN
                                    </span>
                                  ) : (
                                    <span className="text-neutral-600 italic">Usuario</span>
                                  )}
                                </td>
                                <td className="py-4 px-6 text-neutral-400 font-mono text-xs">{user.linkedAt}</td>
                                <td className="py-4 px-6 text-right">
                                  <div className="flex items-center justify-end gap-2 md:opacity-50 group-hover:opacity-100 transition-opacity">
                                    <button
                                      onClick={() => {
                                        setStatsSearch(user.faceitNickname);
                                        setActiveTab('stats');
                                        setTimeout(() => handlePlayerSearch(undefined, user.faceitNickname), 100);
                                      }}
                                      className="px-3 py-1.5 text-[9px] font-mono font-black uppercase tracking-wider bg-[#FF5500]/20 text-[#FF5500] hover:bg-[#FF5500] hover:text-white rounded-none border border-[#FF5500]/30 transition-colors cursor-pointer"
                                    >
                                      Estadísticas
                                    </button>

                                    <button
                                      onClick={() => handleSyncUserNick(user.discordId, user.faceitNickname)}
                                      className="px-3 py-1.5 text-[9px] font-mono font-black uppercase tracking-wider bg-emerald-950/20 text-emerald-400 hover:bg-emerald-600 hover:text-white rounded-none border border-emerald-500/20 transition-colors cursor-pointer"
                                      title="Sincronizar apodo de este usuario en Discord"
                                    >
                                      Sync Nick
                                    </button>

                                    <button
                                      onClick={() => handleOpenBalanceModal(user)}
                                      className="px-3 py-1.5 text-[9px] font-mono font-black uppercase tracking-wider bg-emerald-900/30 text-emerald-300 hover:bg-emerald-600 hover:text-white rounded-none border border-emerald-500/30 transition-colors cursor-pointer"
                                      title="Gestionar Saldo de Billetera"
                                    >
                                      Saldo
                                    </button>

                                    <button
                                      onClick={() => handleOpenRoleModal(user.discordId)}
                                      className="px-3 py-1.5 text-[9px] font-mono font-black uppercase tracking-wider bg-amber-950/20 text-amber-400 hover:bg-amber-600 hover:text-white rounded-none border border-amber-500/20 transition-colors cursor-pointer"
                                      title="Asignar Rol Personalizado en Discord"
                                    >
                                      Rol Discord
                                    </button>

                                    {(currentUser?.role === 'global_admin' || currentUser?.role === 'admin') && (
                                      <button
                                        onClick={() => handleOpenAdminRoleModal(user)}
                                        className="px-3 py-1.5 text-[9px] font-mono font-black uppercase tracking-wider bg-blue-950/20 text-blue-400 hover:bg-blue-600 hover:text-white rounded-none border border-blue-500/20 transition-colors cursor-pointer"
                                        title="Gestionar Permisos Administrativos"
                                      >
                                        Permisos Web
                                      </button>
                                    )}

                                    <button
                                      onClick={() => handleUnlinkUser(user.discordId)}
                                      className="px-3 py-1.5 text-[9px] font-mono font-black uppercase tracking-wider bg-red-950/20 text-red-400 hover:bg-red-600 hover:text-white rounded-none border border-red-500/20 transition-colors cursor-pointer"
                                      title="Desvincular Usuario"
                                    >
                                      Eliminar
                                    </button>
                                  </div>
                                </td>
                              </tr>
                            ))
                          )}
                        </tbody>
                      </table>
                    </div>

                    {/* Table Pagination Footer */}
                    <div className="bg-neutral-900 p-6 border-t border-white/10 flex items-center justify-between">
                      <span className="text-xs text-neutral-400 font-mono uppercase tracking-wider font-bold">
                        Mostrando {filteredUsers.length > 0 ? (usersPage - 1) * 15 + 1 : 0} a {Math.min(filteredUsers.length, usersPage * 15)} de {filteredUsers.length} vinculaciones (Total: {users.length})
                      </span>
                      <div className="flex items-center gap-4">
                        <span className="text-xs font-mono text-neutral-400">PÁGINA {usersPage} DE {totalPages}</span>
                        <div className="flex gap-1">
                          <button
                            onClick={() => setUsersPage(p => Math.max(1, p - 1))}
                            disabled={usersPage === 1}
                            className="p-2 text-neutral-500 hover:text-white disabled:opacity-20 cursor-pointer border border-white/5 hover:border-white/20 transition-colors"
                          >
                            <ChevronLeft className="w-5 h-5" />
                          </button>
                          <button
                            onClick={() => setUsersPage(p => Math.min(totalPages, p + 1))}
                            disabled={usersPage === totalPages}
                            className="p-2 text-neutral-500 hover:text-white disabled:opacity-20 cursor-pointer border border-white/5 hover:border-white/20 transition-colors"
                          >
                            <ChevronRight className="w-5 h-5" />
                          </button>
                        </div>
                      </div>
                    </div>
                  </div>

                </div>
              )}

              {/* VIEW: STORE ORDERS */}
              {activeTab === 'orders' && (
                <div className="space-y-6 animate-fade-in">
                  {/* Top Stats Banner */}
                  <div className="bg-[#111] border-2 border-white/10 p-8 flex flex-col md:flex-row md:items-center justify-between gap-4 rounded-none">
                    <div>
                      <h3 className="text-xl font-black text-white uppercase tracking-tighter italic mb-1 flex items-center gap-2">
                        <ShoppingBag className="w-5 h-5 text-white" />
                        <span>ÓRDENES DE TIENDA Y COMPRAS</span>
                      </h3>
                      <p className="text-xs text-neutral-400 uppercase tracking-tight font-mono">Control de entregas de skins, pedidos de usuarios y transacciones comerciales.</p>
                    </div>

                    <button
                      onClick={() => {
                        showToast('Actualizando órdenes...', 'info');
                        loadData();
                      }}
                      className="flex items-center gap-2 px-5 py-2.5 bg-white text-black hover:bg-neutral-200 text-xs font-black uppercase tracking-tighter rounded-none cursor-pointer"
                    >
                      <RefreshCw className="w-3.5 h-3.5" />
                      <span>Actualizar Datos</span>
                    </button>
                  </div>

                  {/* Summary Metric Cards */}
                  <div className="grid grid-cols-1 md:grid-cols-4 gap-6">
                    <div className="bg-[#111] border-2 border-white/10 p-6">
                      <p className="text-[10px] font-mono text-neutral-400 uppercase tracking-widest font-bold mb-1">TOTAL ÓRDENES</p>
                      <p className="text-3xl font-black text-white font-mono">{orders.length}</p>
                    </div>
                    <div className="bg-[#111] border-2 border-white/10 p-6">
                      <p className="text-[10px] font-mono text-yellow-400 uppercase tracking-widest font-bold mb-1">PENDIENTES / PROCESO</p>
                      <p className="text-3xl font-black text-yellow-400 font-mono">
                        {orders.filter(o => ['PENDIENTE', 'PENDING', 'PROCESANDO'].includes((o.status || '').toUpperCase())).length}
                      </p>
                    </div>
                    <div className="bg-[#111] border-2 border-white/10 p-6">
                      <p className="text-[10px] font-mono text-emerald-400 uppercase tracking-widest font-bold mb-1">COMPLETADAS / ENTREGADAS</p>
                      <p className="text-3xl font-black text-emerald-400 font-mono">
                        {orders.filter(o => ['COMPLETADA', 'ENTREGADO', 'PAGADO', 'COMPLETED'].includes((o.status || '').toUpperCase())).length}
                      </p>
                    </div>
                    <div className="bg-[#111] border-2 border-white/10 p-6">
                      <p className="text-[10px] font-mono text-neutral-400 uppercase tracking-widest font-bold mb-1">VOLUMEN TOTAL</p>
                      <p className="text-3xl font-black text-emerald-400 font-mono">
                        S/ {orders.reduce((acc, o) => acc + (o.totalAmount || (o as any).total || 0), 0).toFixed(2)}
                      </p>
                    </div>
                  </div>

                  {/* Filters Bar */}
                  <div className="flex flex-col md:flex-row gap-4 items-center justify-between">
                    <div className="relative flex-1 w-full">
                      <Search className="w-4 h-4 text-neutral-500 absolute left-4 top-1/2 -translate-y-1/2" />
                      <input
                        type="text"
                        placeholder="BUSCAR POR ID, CLIENTE, EMAIL, REFERENCIA..."
                        value={ordersSearch}
                        onChange={(e) => setOrdersSearch(e.target.value)}
                        className="w-full bg-[#111] border-2 border-white/10 text-xs font-mono font-bold text-white placeholder-neutral-500 rounded-none pl-12 pr-4 py-4 outline-none focus:border-white transition-all uppercase tracking-wider"
                      />
                    </div>
                    <select
                      value={ordersFilter}
                      onChange={(e) => setOrdersFilter(e.target.value)}
                      className="bg-[#111] border-2 border-white/10 text-xs font-mono font-bold text-white rounded-none px-4 py-4 outline-none focus:border-white transition-all uppercase cursor-pointer"
                    >
                      <option value="all">TODOS LOS ESTADOS</option>
                      <option value="PENDIENTE">PENDIENTE</option>
                      <option value="PAGADO">PAGADO</option>
                      <option value="ENTREGADO">ENTREGADO</option>
                      <option value="COMPLETADA">COMPLETADA</option>
                      <option value="CANCELADO">CANCELADO</option>
                    </select>
                  </div>

                  {/* Orders Table */}
                  <div className="bg-[#111] border-2 border-white/10 overflow-hidden shadow-[10px_10px_0px_rgba(0,0,0,0.5)] rounded-none">
                    <div className="overflow-x-auto">
                      <table className="w-full text-left border-collapse">
                        <thead>
                          <tr className="bg-neutral-900 border-b border-white/10">
                            <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">ID Órden</th>
                            <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Cliente</th>
                            <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Productos</th>
                            <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Total (S/.)</th>
                            <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Método / Ref</th>
                            <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Estado</th>
                            <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Fecha</th>
                            <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400 text-right">Acción</th>
                          </tr>
                        </thead>
                        <tbody className="divide-y divide-white/5 text-xs font-mono">
                          {filteredOrders.length === 0 ? (
                            <tr>
                              <td colSpan={8} className="py-8 text-center text-[#8f8fa0] font-sans">
                                No se encontraron órdenes registradas.
                              </td>
                            </tr>
                          ) : (
                            paginatedOrders.map(order => {
                              const orderCode = order.orderId || order.id;
                              const statusUpper = (order.status || '').toUpperCase();
                              return (
                                <tr key={order.id || order.orderId} className="hover:bg-white/[0.02] border-b border-white/5 transition-colors group">
                                  <td className="py-4 px-6 font-mono text-xs font-black text-white">{orderCode}</td>
                                  <td className="py-4 px-6">
                                    <div className="font-black text-white uppercase">{order.customer?.name || 'Cliente'}</div>
                                    {order.customer?.discordId && (
                                      <div className="text-[10px] text-neutral-400 font-mono">ID: {order.customer.discordId}</div>
                                    )}
                                    {order.customer?.email && (
                                      <div className="text-[10px] text-neutral-500 font-mono">{order.customer.email}</div>
                                    )}
                                  </td>
                                  <td className="py-4 px-6">
                                    <div className="max-w-xs space-y-1">
                                      {(order.items || []).map((it, idx) => (
                                        <div key={idx} className="text-[11px] text-neutral-300 font-mono truncate" title={it.name}>
                                          <span className="text-white font-bold">{it.quantity}x</span> {it.name}
                                        </div>
                                      ))}
                                    </div>
                                  </td>
                                  <td className="py-4 px-6 font-mono font-bold text-emerald-400 text-xs">
                                    S/ {(order.totalAmount || (order as any).total || 0).toFixed(2)}
                                  </td>
                                  <td className="py-4 px-6 font-mono text-neutral-400 text-[11px]">
                                    <span className="bg-neutral-800 text-neutral-200 px-2 py-0.5 border border-white/10 uppercase text-[9px] font-bold">
                                      {order.paymentMethod || 'YAPE'}
                                    </span>
                                    <div className="text-[9px] text-neutral-500 mt-1 truncate max-w-[120px]">
                                      {order.referenceCode || order.chargeId || '-'}
                                    </div>
                                  </td>
                                  <td className="py-4 px-6">
                                    <span className={`inline-flex items-center gap-1.5 px-2.5 py-1 text-[9px] font-mono font-black uppercase border ${
                                      statusUpper === 'ENTREGADO' || statusUpper === 'COMPLETADA' || statusUpper === 'COMPLETED' ? 'bg-emerald-950/20 text-emerald-400 border-emerald-500/30' :
                                      statusUpper === 'PAGADO' ? 'bg-blue-950/20 text-blue-400 border-blue-500/30' :
                                      statusUpper === 'CANCELADO' ? 'bg-red-950/20 text-red-400 border-red-500/30' :
                                      'bg-yellow-950/20 text-yellow-400 border-yellow-500/30'
                                    }`}>
                                      {order.status || 'PENDIENTE'}
                                    </span>
                                  </td>
                                  <td className="py-4 px-6 text-neutral-400 font-mono text-[11px]">
                                    {formatDateDisplay(order.createdAtIso || order.createdAt)}
                                  </td>
                                  <td className="py-4 px-6 text-right">
                                    <button
                                      onClick={() => handleOpenOrderModal(order)}
                                      className="px-3 py-1.5 text-[9px] font-mono font-black uppercase tracking-wider bg-white/10 text-white hover:bg-white hover:text-black rounded-none border border-white/20 transition-colors cursor-pointer"
                                    >
                                      Detalles / Estado
                                    </button>
                                  </td>
                                </tr>
                              );
                            })
                          )}
                        </tbody>
                      </table>
                    </div>

                    {/* Table Pagination Footer */}
                    <div className="bg-neutral-900 p-6 border-t border-white/10 flex items-center justify-between">
                      <span className="text-xs text-neutral-400 font-mono uppercase tracking-wider font-bold">
                        Mostrando {filteredOrders.length > 0 ? (ordersPage - 1) * 10 + 1 : 0} a {Math.min(filteredOrders.length, ordersPage * 10)} de {filteredOrders.length} órdenes (Total: {orders.length})
                      </span>
                      <div className="flex items-center gap-4">
                        <span className="text-xs font-mono text-neutral-400">PÁGINA {ordersPage} DE {totalOrdersPages}</span>
                        <div className="flex gap-1">
                          <button
                            onClick={() => setOrdersPage(p => Math.max(1, p - 1))}
                            disabled={ordersPage === 1}
                            className="p-2 text-neutral-500 hover:text-white disabled:opacity-20 cursor-pointer border border-white/5 hover:border-white/20 transition-colors"
                          >
                            <ChevronLeft className="w-5 h-5" />
                          </button>
                          <button
                            onClick={() => setOrdersPage(p => Math.min(totalOrdersPages, p + 1))}
                            disabled={ordersPage === totalOrdersPages}
                            className="p-2 text-neutral-500 hover:text-white disabled:opacity-20 cursor-pointer border border-white/5 hover:border-white/20 transition-colors"
                          >
                            <ChevronRight className="w-5 h-5" />
                          </button>
                        </div>
                      </div>
                    </div>
                  </div>
                </div>
              )}

              {/* VIEW: FINANCES & YAPE */}
              {activeTab === 'finanzas' && (
                <div className="space-y-6 animate-fade-in">
                  {/* Top Stats Banner */}
                  <div className="bg-[#111] border-2 border-white/10 p-8 flex flex-col md:flex-row md:items-center justify-between gap-4 rounded-none">
                    <div>
                      <h3 className="text-xl font-black text-white uppercase tracking-tighter italic mb-1 flex items-center gap-2">
                        <Wallet className="w-5 h-5 text-emerald-400" />
                        <span>FINANZAS, DEPOSITOS Y AUDITORÍA YAPE</span>
                      </h3>
                      <p className="text-xs text-neutral-400 uppercase tracking-tight font-mono">Conciliación de pagos automáticos por OCR/notificaciones y acreditación manual.</p>
                    </div>

                    <div className="flex items-center gap-3">
                      {currentUser?.role === 'global_admin' && (
                        <button
                          onClick={() => handleOpenManualApproveYape()}
                          className="flex items-center gap-2 px-5 py-2.5 bg-emerald-500 text-black hover:bg-emerald-400 text-xs font-black uppercase tracking-tighter rounded-none cursor-pointer"
                        >
                          <Plus className="w-3.5 h-3.5" />
                          <span>Aprobación Manual</span>
                        </button>
                      )}
                      <button
                        onClick={() => {
                          showToast('Actualizando finanzas...', 'info');
                          loadData();
                        }}
                        className="flex items-center gap-2 px-5 py-2.5 bg-white text-black hover:bg-neutral-200 text-xs font-black uppercase tracking-tighter rounded-none cursor-pointer"
                      >
                        <RefreshCw className="w-3.5 h-3.5" />
                        <span>Actualizar Datos</span>
                      </button>
                    </div>
                  </div>

                  {/* Summary Metric Cards */}
                  <div className="grid grid-cols-1 md:grid-cols-4 gap-6">
                    <div className="bg-[#111] border-2 border-white/10 p-6">
                      <p className="text-[10px] font-mono text-neutral-400 uppercase tracking-widest font-bold mb-1">INTENCIONES / RESERVAS</p>
                      <p className="text-3xl font-black text-white font-mono">{yapeData.operations.length}</p>
                    </div>
                    <div className="bg-[#111] border-2 border-white/10 p-6">
                      <p className="text-[10px] font-mono text-emerald-400 uppercase tracking-widest font-bold mb-1">CONFIRMADAS</p>
                      <p className="text-3xl font-black text-emerald-400 font-mono">
                        {yapeData.operations.filter(op => (op.status || '').toUpperCase() === 'CONFIRMED' || (op.status || '').toUpperCase() === 'CONFIRMADO').length}
                      </p>
                    </div>
                    <div className="bg-[#111] border-2 border-white/10 p-6">
                      <p className="text-[10px] font-mono text-purple-400 uppercase tracking-widest font-bold mb-1">PAGOS DETECTADOS (ESCÁNER)</p>
                      <p className="text-3xl font-black text-purple-400 font-mono">{yapeData.transactions.length}</p>
                    </div>
                    <div className="bg-[#111] border-2 border-white/10 p-6">
                      <p className="text-[10px] font-mono text-neutral-400 uppercase tracking-widest font-bold mb-1">LOGS DE AUDITORÍA</p>
                      <p className="text-3xl font-black text-neutral-300 font-mono">{yapeData.auditLogs.length}</p>
                    </div>
                  </div>

                  {/* Subtabs Switcher */}
                  <div className="flex border-b-2 border-white/10 gap-2">
                    <button
                      onClick={() => setYapeActiveSubtab('operations')}
                      className={`px-6 py-3 text-xs font-mono font-black uppercase tracking-wider transition-colors cursor-pointer ${
                        yapeActiveSubtab === 'operations' ? 'bg-white text-black border-t-2 border-white' : 'text-neutral-400 hover:text-white bg-[#111]'
                      }`}
                    >
                      Reservas de Depósito ({yapeData.operations.length})
                    </button>
                    <button
                      onClick={() => setYapeActiveSubtab('transactions')}
                      className={`px-6 py-3 text-xs font-mono font-black uppercase tracking-wider transition-colors cursor-pointer ${
                        yapeActiveSubtab === 'transactions' ? 'bg-white text-black border-t-2 border-white' : 'text-neutral-400 hover:text-white bg-[#111]'
                      }`}
                    >
                      Notificaciones Escáner ({yapeData.transactions.length})
                    </button>
                    <button
                      onClick={() => setYapeActiveSubtab('audit')}
                      className={`px-6 py-3 text-xs font-mono font-black uppercase tracking-wider transition-colors cursor-pointer ${
                        yapeActiveSubtab === 'audit' ? 'bg-white text-black border-t-2 border-white' : 'text-neutral-400 hover:text-white bg-[#111]'
                      }`}
                    >
                      Auditoría de Saldos ({yapeData.auditLogs.length})
                    </button>
                  </div>

                  {/* SUBTAB 1: OPERACIONES / RESERVAS */}
                  {yapeActiveSubtab === 'operations' && (
                    <div className="bg-[#111] border-2 border-white/10 overflow-hidden shadow-[10px_10px_0px_rgba(0,0,0,0.5)] rounded-none">
                      <div className="overflow-x-auto">
                        <table className="w-full text-left border-collapse">
                          <thead>
                            <tr className="bg-neutral-900 border-b border-white/10">
                              <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">ID Reserva</th>
                              <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Discord ID</th>
                              <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Monto</th>
                              <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">N° Operación</th>
                              <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Estado</th>
                              <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Fecha Creación</th>
                              <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400 text-right">Acción</th>
                            </tr>
                          </thead>
                          <tbody className="divide-y divide-white/5 text-xs font-mono">
                            {yapeData.operations.length === 0 ? (
                              <tr>
                                <td colSpan={7} className="py-8 text-center text-[#8f8fa0] font-sans">
                                  No hay reservas de depósito registradas.
                                </td>
                              </tr>
                            ) : (
                              yapeData.operations.map(op => {
                                const isPending = (op.status || '').toUpperCase() === 'PENDING' || (op.status || '').toUpperCase() === 'PENDIENTE';
                                return (
                                  <tr key={op.id} className="hover:bg-white/[0.02] border-b border-white/5 transition-colors">
                                    <td className="py-4 px-6 font-mono text-white font-bold">{op.id}</td>
                                    <td className="py-4 px-6 font-mono text-neutral-300">{op.discordId || 'N/A'}</td>
                                    <td className="py-4 px-6 font-mono text-emerald-400 font-black">
                                      S/ {(op.amount || 0).toFixed(2)}
                                    </td>
                                    <td className="py-4 px-6 font-mono text-neutral-300 font-bold">
                                      {op.operationNumber || '-'}
                                    </td>
                                    <td className="py-4 px-6">
                                      <span className={`inline-flex items-center gap-1.5 px-2.5 py-1 text-[9px] font-mono font-black uppercase border ${
                                        (op.status || '').toUpperCase() === 'CONFIRMED' ? 'bg-emerald-950/20 text-emerald-400 border-emerald-500/30' :
                                        (op.status || '').toUpperCase() === 'EXPIRED' ? 'bg-neutral-900 text-neutral-500 border-white/10' :
                                        'bg-yellow-950/20 text-yellow-400 border-yellow-500/30'
                                      }`}>
                                        {op.status || 'PENDING'}
                                      </span>
                                    </td>
                                    <td className="py-4 px-6 text-neutral-400 font-mono text-[11px]">
                                      {formatDateDisplay(op.createdAt)}
                                    </td>
                                    <td className="py-4 px-6 text-right">
                                      {isPending && currentUser?.role === 'global_admin' && (
                                        <button
                                          onClick={() => handleOpenManualApproveYape({
                                            op: op.operationNumber || op.id,
                                            discordId: op.discordId,
                                            amount: op.amount
                                          })}
                                          className="px-3 py-1.5 text-[9px] font-mono font-black uppercase tracking-wider bg-emerald-950/20 text-emerald-400 hover:bg-emerald-600 hover:text-white rounded-none border border-emerald-500/30 transition-colors cursor-pointer"
                                        >
                                          Validar
                                        </button>
                                      )}
                                    </td>
                                  </tr>
                                );
                              })
                            )}
                          </tbody>
                        </table>
                      </div>
                    </div>
                  )}

                  {/* SUBTAB 2: TRANSACCIONES ESCÁNER */}
                  {yapeActiveSubtab === 'transactions' && (
                    <div className="bg-[#111] border-2 border-white/10 overflow-hidden shadow-[10px_10px_0px_rgba(0,0,0,0.5)] rounded-none">
                      <div className="overflow-x-auto">
                        <table className="w-full text-left border-collapse">
                          <thead>
                            <tr className="bg-neutral-900 border-b border-white/10">
                              <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">ID / Hash</th>
                              <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Remitente</th>
                              <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Monto</th>
                              <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">N° Operación</th>
                              <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Estado</th>
                              <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Detectado</th>
                            </tr>
                          </thead>
                          <tbody className="divide-y divide-white/5 text-xs font-mono">
                            {yapeData.transactions.length === 0 ? (
                              <tr>
                                <td colSpan={6} className="py-8 text-center text-[#8f8fa0] font-sans">
                                  No hay notificaciones de pagos capturadas por el escáner.
                                </td>
                              </tr>
                            ) : (
                              yapeData.transactions.map(tx => (
                                <tr key={tx.id} className="hover:bg-white/[0.02] border-b border-white/5 transition-colors">
                                  <td className="py-4 px-6 font-mono text-white text-[11px] font-bold">{tx.id}</td>
                                  <td className="py-4 px-6 font-mono text-white uppercase">{tx.sender || 'YAPERO ANÓNIMO'}</td>
                                  <td className="py-4 px-6 font-mono text-emerald-400 font-black">
                                    S/ {(tx.amount || tx.monto || 0).toFixed(2)}
                                  </td>
                                  <td className="py-4 px-6 font-mono text-purple-400 font-bold">
                                    {tx.operationNumber || tx.numero_operacion || '-'}
                                  </td>
                                  <td className="py-4 px-6">
                                    <span className="bg-neutral-800 text-neutral-200 px-2 py-0.5 border border-white/10 uppercase text-[9px] font-bold">
                                      {tx.status || 'DETECTADO'}
                                    </span>
                                  </td>
                                  <td className="py-4 px-6 text-neutral-400 font-mono text-[11px]">
                                    {formatDateDisplay(tx.detectedAt || tx.createdAt)}
                                  </td>
                                </tr>
                              ))
                            )}
                          </tbody>
                        </table>
                      </div>
                    </div>
                  )}

                  {/* SUBTAB 3: AUDITORÍA DE SALDOS */}
                  {yapeActiveSubtab === 'audit' && (
                    <div className="bg-[#111] border-2 border-white/10 overflow-hidden shadow-[10px_10px_0px_rgba(0,0,0,0.5)] rounded-none">
                      <div className="overflow-x-auto">
                        <table className="w-full text-left border-collapse">
                          <thead>
                            <tr className="bg-neutral-900 border-b border-white/10">
                              <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Timestamp</th>
                              <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Acción</th>
                              <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Discord ID</th>
                              <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Monto (S/.)</th>
                              <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Motivo / Razón</th>
                              <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Origen / Admin</th>
                            </tr>
                          </thead>
                          <tbody className="divide-y divide-white/5 text-xs font-mono">
                            {yapeData.auditLogs.length === 0 ? (
                              <tr>
                                <td colSpan={6} className="py-8 text-center text-[#8f8fa0] font-sans">
                                  No hay registros de auditoría de saldo.
                                </td>
                              </tr>
                            ) : (
                              yapeData.auditLogs.map(log => (
                                <tr key={log.id} className="hover:bg-white/[0.02] border-b border-white/5 transition-colors">
                                  <td className="py-4 px-6 text-neutral-400 font-mono text-[11px]">
                                    {formatDateDisplay(log.timestamp)}
                                  </td>
                                  <td className="py-4 px-6">
                                    <span className="bg-blue-950/30 text-blue-300 border border-blue-500/20 px-2 py-0.5 text-[9px] font-mono font-black uppercase">
                                      {log.action || 'AJUSTE'}
                                    </span>
                                  </td>
                                  <td className="py-4 px-6 font-mono text-white font-bold">{log.discordId || 'N/A'}</td>
                                  <td className="py-4 px-6 font-mono text-emerald-400 font-black">
                                    S/ {(log.amount || 0).toFixed(2)}
                                  </td>
                                  <td className="py-4 px-6 text-neutral-300 font-mono text-xs">{log.reason || '-'}</td>
                                  <td className="py-4 px-6 text-neutral-400 font-mono text-[11px]">{log.source || 'ADMIN'}</td>
                                </tr>
                              ))
                            )}
                          </tbody>
                        </table>
                      </div>
                    </div>
                  )}
                </div>
              )}

              {/* VIEW: INDECOPI CLAIMS */}
              {activeTab === 'reclamaciones' && (
                <div className="space-y-6 animate-fade-in">
                  {/* Top Stats Banner */}
                  <div className="bg-[#111] border-2 border-white/10 p-8 flex flex-col md:flex-row md:items-center justify-between gap-4 rounded-none">
                    <div>
                      <h3 className="text-xl font-black text-white uppercase tracking-tighter italic mb-1 flex items-center gap-2">
                        <Scale className="w-5 h-5 text-amber-400" />
                        <span>LIBRO DE RECLAMACIONES VIRTUAL (INDECOPI - LEY Nº 29571)</span>
                      </h3>
                      <p className="text-xs text-neutral-400 uppercase tracking-tight font-mono">Gestión obligatoria de quejas y reclamos de consumidores con plazo legal improrrogable de 15 días hábiles.</p>
                    </div>

                    <button
                      onClick={() => {
                        showToast('Actualizando reclamaciones...', 'info');
                        loadData();
                      }}
                      className="flex items-center gap-2 px-5 py-2.5 bg-white text-black hover:bg-neutral-200 text-xs font-black uppercase tracking-tighter rounded-none cursor-pointer"
                    >
                      <RefreshCw className="w-3.5 h-3.5" />
                      <span>Actualizar Datos</span>
                    </button>
                  </div>

                  {/* Summary Metric Cards */}
                  <div className="grid grid-cols-1 md:grid-cols-4 gap-6">
                    <div className="bg-[#111] border-2 border-white/10 p-6">
                      <p className="text-[10px] font-mono text-neutral-400 uppercase tracking-widest font-bold mb-1">TOTAL RECLAMOS</p>
                      <p className="text-3xl font-black text-white font-mono">{claims.length}</p>
                    </div>
                    <div className="bg-[#111] border-2 border-white/10 p-6">
                      <p className="text-[10px] font-mono text-yellow-400 uppercase tracking-widest font-bold mb-1">PENDIENTES</p>
                      <p className="text-3xl font-black text-yellow-400 font-mono">
                        {claims.filter(c => (c.status || '').toUpperCase() === 'PENDIENTE').length}
                      </p>
                    </div>
                    <div className="bg-[#111] border-2 border-white/10 p-6">
                      <p className="text-[10px] font-mono text-blue-400 uppercase tracking-widest font-bold mb-1">EN PROCESO</p>
                      <p className="text-3xl font-black text-blue-400 font-mono">
                        {claims.filter(c => (c.status || '').toUpperCase() === 'EN_PROCESO').length}
                      </p>
                    </div>
                    <div className="bg-[#111] border-2 border-white/10 p-6">
                      <p className="text-[10px] font-mono text-emerald-400 uppercase tracking-widest font-bold mb-1">ATENDIDOS / RESUELTOS</p>
                      <p className="text-3xl font-black text-emerald-400 font-mono">
                        {claims.filter(c => (c.status || '').toUpperCase() === 'ATENDIDO').length}
                      </p>
                    </div>
                  </div>

                  {/* Filters Bar */}
                  <div className="flex flex-col md:flex-row gap-4 items-center justify-between">
                    <div className="relative flex-1 w-full">
                      <Search className="w-4 h-4 text-neutral-500 absolute left-4 top-1/2 -translate-y-1/2" />
                      <input
                        type="text"
                        placeholder="BUSCAR POR CÓDIGO, NOMBRE, DNI O EMAIL..."
                        value={claimsSearch}
                        onChange={(e) => setClaimsSearch(e.target.value)}
                        className="w-full bg-[#111] border-2 border-white/10 text-xs font-mono font-bold text-white placeholder-neutral-500 rounded-none pl-12 pr-4 py-4 outline-none focus:border-white transition-all uppercase tracking-wider"
                      />
                    </div>
                    <select
                      value={claimsFilter}
                      onChange={(e) => setClaimsFilter(e.target.value)}
                      className="bg-[#111] border-2 border-white/10 text-xs font-mono font-bold text-white rounded-none px-4 py-4 outline-none focus:border-white transition-all uppercase cursor-pointer"
                    >
                      <option value="all">TODOS LOS ESTADOS</option>
                      <option value="PENDIENTE">PENDIENTE</option>
                      <option value="EN_PROCESO">EN PROCESO</option>
                      <option value="ATENDIDO">ATENDIDO</option>
                      <option value="RECHAZADO">RECHAZADO</option>
                    </select>
                  </div>

                  {/* Claims Table */}
                  <div className="bg-[#111] border-2 border-white/10 overflow-hidden shadow-[10px_10px_0px_rgba(0,0,0,0.5)] rounded-none">
                    <div className="overflow-x-auto">
                      <table className="w-full text-left border-collapse">
                        <thead>
                          <tr className="bg-neutral-900 border-b border-white/10">
                            <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Código</th>
                            <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Consumidor</th>
                            <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Tipo</th>
                            <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Bien Reclamado</th>
                            <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Monto</th>
                            <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Estado</th>
                            <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Fecha Registro</th>
                            <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400 text-right">Acción</th>
                          </tr>
                        </thead>
                        <tbody className="divide-y divide-white/5 text-xs font-mono">
                          {filteredClaims.length === 0 ? (
                            <tr>
                              <td colSpan={8} className="py-8 text-center text-[#8f8fa0] font-sans">
                                No se encontraron reclamaciones registradas.
                              </td>
                            </tr>
                          ) : (
                            paginatedClaims.map(claim => {
                              const code = claim.claimCode || claim.id;
                              const statusUpper = (claim.status || '').toUpperCase();
                              return (
                                <tr key={code} className="hover:bg-white/[0.02] border-b border-white/5 transition-colors group">
                                  <td className="py-4 px-6 font-mono text-xs font-black text-white">{code}</td>
                                  <td className="py-4 px-6">
                                    <div className="font-black text-white uppercase">{claim.consumer?.name || 'Consumidor'}</div>
                                    <div className="text-[10px] text-neutral-400 font-mono">
                                      {claim.consumer?.docType || 'DNI'}: {claim.consumer?.docNumber || '-'}
                                    </div>
                                    <div className="text-[10px] text-neutral-500 font-mono">
                                      {claim.consumer?.email || '-'} {claim.consumer?.phone ? `| ${claim.consumer.phone}` : ''}
                                    </div>
                                  </td>
                                  <td className="py-4 px-6">
                                    <span className={`px-2 py-0.5 text-[9px] font-mono font-black uppercase border ${
                                      claim.claim?.type === 'QUEJA' ? 'bg-purple-950/30 text-purple-300 border-purple-500/30' : 'bg-amber-950/30 text-amber-300 border-amber-500/30'
                                    }`}>
                                      {claim.claim?.type || 'RECLAMO'}
                                    </span>
                                  </td>
                                  <td className="py-4 px-6 font-mono text-neutral-300">
                                    <span className="text-white font-bold">{claim.goods?.type || 'PRODUCTO'}</span>
                                    <div className="text-[10px] text-neutral-400 truncate max-w-[180px]" title={claim.goods?.description}>
                                      {claim.goods?.description || '-'}
                                    </div>
                                  </td>
                                  <td className="py-4 px-6 font-mono font-bold text-emerald-400 text-xs">
                                    S/ {(claim.goods?.amount || 0).toFixed(2)}
                                  </td>
                                  <td className="py-4 px-6">
                                    <span className={`inline-flex items-center gap-1.5 px-2.5 py-1 text-[9px] font-mono font-black uppercase border ${
                                      statusUpper === 'ATENDIDO' ? 'bg-emerald-950/20 text-emerald-400 border-emerald-500/30' :
                                      statusUpper === 'EN_PROCESO' ? 'bg-blue-950/20 text-blue-400 border-blue-500/30' :
                                      statusUpper === 'RECHAZADO' ? 'bg-red-950/20 text-red-400 border-red-500/30' :
                                      'bg-yellow-950/20 text-yellow-400 border-yellow-500/30'
                                    }`}>
                                      {claim.status || 'PENDIENTE'}
                                    </span>
                                  </td>
                                  <td className="py-4 px-6 text-neutral-400 font-mono text-[11px]">
                                    {formatDateDisplay(claim.createdAtIso || claim.createdAt)}
                                  </td>
                                  <td className="py-4 px-6 text-right">
                                    <div className="flex items-center justify-end gap-2">
                                      <button
                                        onClick={() => handleOpenClaimModal(claim)}
                                        className="px-3 py-1.5 text-[9px] font-mono font-black uppercase tracking-wider bg-white/10 text-white hover:bg-white hover:text-black rounded-none border border-white/20 transition-colors cursor-pointer"
                                      >
                                        Ver / Responder
                                      </button>
                                      {currentUser?.role === 'global_admin' && (
                                        <button
                                          onClick={() => handleDeleteClaim(code)}
                                          className="px-2.5 py-1.5 text-[9px] font-mono font-black uppercase tracking-wider bg-red-950/20 text-red-400 hover:bg-red-600 hover:text-white rounded-none border border-red-500/20 transition-colors cursor-pointer"
                                          title="Eliminar Reclamación"
                                        >
                                          <Trash2 className="w-3 h-3" />
                                        </button>
                                      )}
                                    </div>
                                  </td>
                                </tr>
                              );
                            })
                          )}
                        </tbody>
                      </table>
                    </div>

                    {/* Table Pagination Footer */}
                    <div className="bg-neutral-900 p-6 border-t border-white/10 flex items-center justify-between">
                      <span className="text-xs text-neutral-400 font-mono uppercase tracking-wider font-bold">
                        Mostrando {filteredClaims.length > 0 ? (claimsPage - 1) * 10 + 1 : 0} a {Math.min(filteredClaims.length, claimsPage * 10)} de {filteredClaims.length} reclamaciones (Total: {claims.length})
                      </span>
                      <div className="flex items-center gap-4">
                        <span className="text-xs font-mono text-neutral-400">PÁGINA {claimsPage} DE {totalClaimsPages}</span>
                        <div className="flex gap-1">
                          <button
                            onClick={() => setClaimsPage(p => Math.max(1, p - 1))}
                            disabled={claimsPage === 1}
                            className="p-2 text-neutral-500 hover:text-white disabled:opacity-20 cursor-pointer border border-white/5 hover:border-white/20 transition-colors"
                          >
                            <ChevronLeft className="w-5 h-5" />
                          </button>
                          <button
                            onClick={() => setClaimsPage(p => Math.min(totalClaimsPages, p + 1))}
                            disabled={claimsPage === totalClaimsPages}
                            className="p-2 text-neutral-500 hover:text-white disabled:opacity-20 cursor-pointer border border-white/5 hover:border-white/20 transition-colors"
                          >
                            <ChevronRight className="w-5 h-5" />
                          </button>
                        </div>
                      </div>
                    </div>
                  </div>
                </div>
              )}

              {/* VIEW 3: LIVE MATCHES */}
              {activeTab === 'matches' && (
                <div className="space-y-6 animate-fade-in">
                  
                  {/* Page header and general state */}
                  <div className="bg-[#111] border-2 border-white/10 p-8 flex flex-col md:flex-row md:items-center justify-between gap-4 rounded-none">
                    <div>
                      <h3 className="text-xl font-black text-white uppercase tracking-tighter italic mb-1">PARTIDAS ACTIVAS EN EL HUB</h3>
                      <p className="text-xs text-neutral-400 uppercase tracking-tight font-mono">Monitoreo en tiempo real de instancias de CS2 hospedadas en servidores.</p>
                    </div>

                    <button
                      onClick={() => {
                        showToast('Refrescando partidas activas...', 'info');
                        loadData();
                      }}
                      className="flex items-center gap-2 px-5 py-2.5 bg-white text-black hover:bg-neutral-200 text-xs font-black uppercase tracking-tighter rounded-none cursor-pointer"
                    >
                      <RefreshCw className="w-3.5 h-3.5" />
                      <span>Actualizar Datos</span>
                    </button>
                  </div>

                  {/* Live match statistics panel */}
                  <div className="grid grid-cols-1 md:grid-cols-3 gap-6">
                    <div className="bg-[#111] border-2 border-white/10 p-6 rounded-none relative">
                      <div className="flex justify-between items-start mb-4 text-neutral-400">
                        <span className="text-[10px] font-mono uppercase tracking-widest font-black">CARGA DE SERVIDORES</span>
                        <Activity className="w-5 h-5 text-white" />
                      </div>
                      <h4 className="text-3xl font-black text-white font-mono leading-none">{matchStats.serverLoad}<span className="text-sm text-neutral-500 font-normal">%</span></h4>
                    </div>

                    <div className="bg-[#111] border-2 border-white/10 p-6 rounded-none relative">
                      <div className="flex justify-between items-start mb-4 text-neutral-400">
                        <span className="text-[10px] font-mono uppercase tracking-widest font-black">DURACIÓN PROMEDIO</span>
                        <Clock className="w-5 h-5 text-white" />
                      </div>
                      <h4 className="text-3xl font-black text-white font-mono leading-none">{matchStats.avgDurationStr}</h4>
                    </div>

                    <div className="bg-[#111] border-2 border-red-500 p-6 rounded-none relative">
                      <div className="flex justify-between items-start mb-4 text-red-500">
                        <span className="text-[10px] font-mono uppercase tracking-widest font-black">LIMPIEZAS FALLIDAS</span>
                        <ShieldAlert className="w-5 h-5" />
                      </div>
                      <h4 className="text-3xl font-black text-red-500 font-mono leading-none">{matchStats.failedCleanups}</h4>
                    </div>
                  </div>

                  {/* Active matches list table */}
                  <div className="bg-[#111] border-2 border-white/10 overflow-hidden shadow-[10px_10px_0px_rgba(0,0,0,0.5)] rounded-none">
                    <div className="overflow-x-auto">
                      <table className="w-full text-left border-collapse">
                        <thead>
                          <tr className="bg-neutral-900 border-b border-white/10">
                            <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">ID Partida</th>
                            <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Equipo A</th>
                            <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Equipo B</th>
                            <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Mapa</th>
                            <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Ubicación</th>
                            <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Iniciada hace</th>
                            <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400 text-right">Acción</th>
                          </tr>
                        </thead>
                        <tbody className="divide-y divide-white/5 text-xs font-mono">
                          {matches.length === 0 ? (
                            <tr>
                              <td colSpan={7} className="py-8 text-center text-neutral-500 font-mono uppercase font-bold">
                                No hay ninguna partida en vivo reportada por Faceit API en este momento.
                              </td>
                            </tr>
                          ) : (
                            matches.map(m => {
                              const isLong = m.startedAtMinutesAgo >= 60;
                              return (
                                <tr key={m.id} className={`hover:bg-white/[0.02] border-b border-white/5 transition-colors group ${isLong ? 'bg-red-950/10' : ''}`}>
                                  <td className="py-5 px-6 font-mono font-black text-white">{m.id}</td>
                                  <td className="py-5 px-6">
                                    <div className="flex items-center gap-2">
                                      {m.teamALogo ? (
                                        <img src={m.teamALogo} alt={m.teamAName} className="w-6 h-6 rounded-none bg-black/40 p-0.5 object-contain" />
                                      ) : (
                                        <div className="w-6 h-6 rounded-none bg-neutral-800 flex items-center justify-center font-black text-[9px] text-white uppercase">
                                          {m.teamAName.substring(0, 3)}
                                        </div>
                                      )}
                                      <span className="font-black text-white uppercase">{m.teamAName}</span>
                                    </div>
                                  </td>
                                  <td className="py-5 px-6">
                                    <div className="flex items-center gap-2">
                                      {m.teamBLogo ? (
                                        <img src={m.teamBLogo} alt={m.teamBName} className="w-6 h-6 rounded-none bg-black/40 p-0.5 object-contain" />
                                      ) : (
                                        <div className="w-6 h-6 rounded-none bg-neutral-800 flex items-center justify-center font-black text-[9px] text-white uppercase">
                                          {m.teamBName.substring(0, 3)}
                                        </div>
                                      )}
                                      <span className="font-black text-white uppercase">{m.teamBName}</span>
                                    </div>
                                  </td>
                                  <td className="py-5 px-6">
                                    <span className="inline-flex items-center gap-1.5 px-2.5 py-1 rounded-none bg-[#1C1C1C] border border-white/10 text-[9px] font-mono text-white font-black uppercase">
                                      <Map className="w-3 h-3 text-white" /> {m.map}
                                    </span>
                                  </td>
                                  <td className="py-5 px-6 text-neutral-400 uppercase font-bold text-[11px]">{m.location}</td>
                                  <td className="py-5 px-6">
                                    {isLong ? (
                                      <span className="text-red-400 font-black font-mono flex items-center gap-1 uppercase">
                                        <Clock className="w-3.5 h-3.5 animate-pulse text-red-500" />
                                        hace {m.startedAtMinutesAgo}m (Larga)
                                      </span>
                                    ) : (
                                      <span className="text-neutral-400 font-mono">hace {m.startedAtMinutesAgo}m</span>
                                    )}
                                  </td>
                                  <td className="py-5 px-6 text-right">
                                    <div className="flex items-center justify-end gap-2">
                                      <a
                                        href={`https://www.faceit.com/en/cs2/room/${m.id}`}
                                        target="_blank"
                                        rel="noreferrer"
                                        className="px-3 py-1.5 rounded-none font-mono text-[9px] font-black uppercase transition-colors cursor-pointer whitespace-nowrap border border-white/10 bg-[#1C1C1C] text-neutral-400 hover:text-white flex items-center gap-1"
                                      >
                                        Ver Room <ArrowUpRight className="w-3 h-3" />
                                      </a>
                                      {(m.id.startsWith('mixton_') || m.id.startsWith('manual_')) && (
                                        <button
                                          onClick={() => handleAnnounceMixtonMatch(m.id)}
                                          className="px-3 py-1.5 rounded-none font-mono text-[9px] font-black uppercase transition-colors cursor-pointer whitespace-nowrap border border-[#FF5500]/30 bg-[#FF5500]/20 text-[#FF5500] hover:bg-[#FF5500] hover:text-white flex items-center gap-1"
                                          title="Enviar anuncio de mixton iniciado con botones de resultado"
                                        >
                                          Enviar Anuncio
                                        </button>
                                      )}
                                      <button
                                        onClick={() => handleCleanupMatch(m.id)}
                                        className={`px-3 py-1.5 rounded-none font-mono text-[9px] font-black uppercase transition-colors cursor-pointer whitespace-nowrap border ${
                                          isLong
                                            ? 'bg-red-950/20 text-red-400 hover:bg-red-600 hover:text-white border-red-500/20'
                                            : 'bg-neutral-800 text-white hover:bg-white hover:text-black border-white/10 md:opacity-50 group-hover:opacity-100'
                                        }`}
                                      >
                                        Forzar Limpieza
                                      </button>
                                    </div>
                                  </td>
                                </tr>
                              );
                            })
                          )}
                        </tbody>
                      </table>
                    </div>
                  </div>
                </div>
              )}

              {/* VIEW: HISTORIAL DE PARTIDAS */}
              {activeTab === 'match-history' && (
                <div className="space-y-6 animate-fade-in">
                  <div className="bg-[#111] border-2 border-white/10 p-8 flex flex-col md:flex-row md:items-center justify-between gap-4 rounded-none">
                    <div>
                      <h3 className="text-xl font-black text-white uppercase tracking-tighter italic mb-1">HISTORIAL DE PARTIDAS</h3>
                      <p className="text-xs text-neutral-400 uppercase tracking-tight font-mono">Registro histórico de partidas en el club (en configuración, en juego, finalizadas y canceladas).</p>
                    </div>
                  </div>

                  <div className="bg-[#111] border-2 border-white/10 overflow-hidden shadow-[10px_10px_0px_rgba(0,0,0,0.5)] rounded-none font-mono">
                    <div className="overflow-x-auto">
                      <table className="w-full text-left border-collapse text-xs">
                        <thead>
                          <tr className="bg-neutral-900 border-b border-white/10 text-neutral-400 font-bold">
                            <th className="py-4 px-6 text-[10px] font-black uppercase tracking-widest w-40">FECHA/HORA</th>
                            <th className="py-4 px-6 text-[10px] font-black uppercase tracking-widest w-44">ID PARTIDA</th>
                            <th className="py-4 px-6 text-[10px] font-black uppercase tracking-widest w-32">ESTADO</th>
                            <th className="py-4 px-6 text-[10px] font-black uppercase tracking-widest">ENFRENTAMIENTO Y MARCADOR</th>
                            <th className="py-4 px-6 text-[10px] font-black uppercase tracking-widest w-32">MAPA</th>
                            <th className="py-4 px-6 text-[10px] font-black uppercase tracking-widest w-36">SERVIDOR</th>
                            <th className="py-4 px-6 text-[10px] font-black uppercase tracking-widest w-40 text-right">ACCIÓN</th>
                          </tr>
                        </thead>
                        <tbody className="divide-y divide-white/5 text-white/90 font-mono">
                          {matchHistory.length === 0 ? (
                            <tr>
                              <td colSpan={7} className="py-8 text-center text-neutral-500 uppercase tracking-widest text-[10px] font-bold">
                                No hay partidas registradas en el historial.
                              </td>
                            </tr>
                          ) : (
                            matchHistory.map((m: any) => {
                              let dateStr = 'Desconocido';
                              if (m.createdAt && m.createdAt._seconds) {
                                dateStr = new Date(m.createdAt._seconds * 1000).toLocaleString('es-ES');
                              } else if (m.createdAt) {
                                dateStr = new Date(m.createdAt).toLocaleString('es-ES');
                              }
                              
                              const isWinnerA = m.winner === 'faction1';
                              const isWinnerB = m.winner === 'faction2';
                              const statusLower = (m.status || '').toLowerCase();

                              let statusBadge = (
                                <span className="px-2 py-0.5 text-[9px] bg-emerald-500/20 text-emerald-400 border border-emerald-500/30 rounded font-mono font-bold">FINALIZADA</span>
                              );
                              if (statusLower === 'configuring') {
                                statusBadge = (
                                  <span className="px-2 py-0.5 text-[9px] bg-yellow-500/20 text-yellow-400 border border-yellow-500/30 rounded font-mono font-bold">CONFIGURANDO</span>
                                );
                              } else if (statusLower === 'started' || statusLower === 'starting') {
                                statusBadge = (
                                  <span className="px-2 py-0.5 text-[9px] bg-blue-500/20 text-blue-400 border border-blue-500/30 rounded font-mono font-bold animate-pulse">EN JUEGO</span>
                                );
                              } else if (statusLower === 'cancelled' || m.winner === 'CANCELLED') {
                                statusBadge = (
                                  <span className="px-2 py-0.5 text-[9px] bg-red-500/20 text-red-400 border border-red-500/30 rounded font-mono font-bold">CANCELADA</span>
                                );
                              }

                              return (
                                <tr key={m.id} className="hover:bg-white/5 transition-colors">
                                  <td className="py-4 px-6 text-neutral-400">{dateStr}</td>
                                  <td className="py-4 px-6 font-bold text-xs">
                                    {m.id.startsWith('mixton_') || m.id.startsWith('manual_') ? (
                                      <span className="text-neutral-400 font-mono">{m.id}</span>
                                    ) : (
                                      <a href={`https://www.faceit.com/en/cs2/room/${m.id}`} target="_blank" rel="noreferrer" className="text-[#FF5500] hover:underline font-mono">
                                        {m.id}
                                      </a>
                                    )}
                                  </td>
                                  <td className="py-4 px-6 whitespace-nowrap">{statusBadge}</td>
                                  <td className="py-4 px-6">
                                    <div className="flex items-center gap-1">
                                      <span className={isWinnerA ? 'text-green-400 font-bold' : ''}>{m.faction1_name}</span>
                                      <span className="text-neutral-500">({m.faction1_score})</span>
                                      <span className="text-neutral-600 mx-1">vs</span>
                                      <span className={isWinnerB ? 'text-green-400 font-bold' : ''}>{m.faction2_name}</span>
                                      <span className="text-neutral-500">({m.faction2_score})</span>
                                    </div>
                                    <div className="text-[10px] text-neutral-500 mt-1 font-mono">
                                      <div className="flex flex-col gap-0.5">
                                        <div>
                                          <span className="text-neutral-400 font-bold">{m.faction1_name}:</span> {m.faction1_roster?.map((p: any) => p.nickname).join(', ') || 'N/A'}
                                        </div>
                                        <div>
                                          <span className="text-neutral-400 font-bold">{m.faction2_name}:</span> {m.faction2_roster?.map((p: any) => p.nickname).join(', ') || 'N/A'}
                                        </div>
                                      </div>
                                    </div>
                                  </td>
                                  <td className="py-4 px-6 uppercase">{m.map}</td>
                                  <td className="py-4 px-6 uppercase text-neutral-400">{m.location}</td>
                                  <td className="py-4 px-6 text-right whitespace-nowrap">
                                    <div className="flex items-center justify-end gap-1.5">
                                      {/* 1. Descargar partida / demo */}
                                      <button
                                        onClick={() => handleDownloadMatchDemo(m)}
                                        disabled={downloadingDemoId === m.id}
                                        className="p-1.5 rounded-none border border-blue-500/30 bg-blue-500/10 text-blue-400 hover:bg-blue-500 hover:text-white transition-colors cursor-pointer flex items-center gap-1 text-[10px] font-bold"
                                        title="Descargar Demo / Partida"
                                      >
                                        <Download className={`w-3.5 h-3.5 ${downloadingDemoId === m.id ? 'animate-bounce' : ''}`} />
                                        <span className="hidden xl:inline">Demo</span>
                                      </button>

                                      {/* 2. Ver estadísticas de la partida */}
                                      <button
                                        onClick={() => handleViewMatchStats(m)}
                                        className="p-1.5 rounded-none border border-emerald-500/30 bg-emerald-500/10 text-emerald-400 hover:bg-emerald-500 hover:text-white transition-colors cursor-pointer flex items-center gap-1 text-[10px] font-bold"
                                        title="Ver estadísticas de la partida"
                                      >
                                        <BarChart3 className="w-3.5 h-3.5" />
                                        <span className="hidden xl:inline">Stats</span>
                                      </button>

                                      {/* 3. Reenviar Resumen (si es mixton) */}
                                      {(m.id.startsWith('mixton_') || m.id.startsWith('manual_')) && (
                                        <button
                                          onClick={() => handleAnnounceMixtonMatch(m.id)}
                                          className="p-1.5 rounded-none border border-white/10 bg-[#1C1C1C] text-neutral-300 hover:bg-white hover:text-black transition-colors cursor-pointer flex items-center gap-1 text-[10px] font-bold"
                                          title="Reenviar resumen al canal de Discord"
                                        >
                                          <RefreshCw className="w-3.5 h-3.5" />
                                          <span className="hidden xl:inline">Resumen</span>
                                        </button>
                                      )}

                                      {/* 4. Eliminar la partida del historial */}
                                      <button
                                        onClick={() => setMatchToDelete(m)}
                                        className="p-1.5 rounded-none border border-red-500/30 bg-red-500/10 text-red-400 hover:bg-red-500 hover:text-white transition-colors cursor-pointer flex items-center gap-1 text-[10px] font-bold"
                                        title="Eliminar partida del historial"
                                      >
                                        <Trash2 className="w-3.5 h-3.5" />
                                        <span className="hidden xl:inline">Eliminar</span>
                                      </button>
                                    </div>
                                  </td>
                                </tr>
                              );
                            })
                          )}
                        </tbody>
                      </table>
                    </div>
                  </div>
                </div>
              )}

              {/* VIEW 4: LEADERBOARD */}
              {activeTab === 'leaderboard' && (
                <div className="space-y-6 animate-fade-in">
                  
                  {/* Leaderboard intro */}
                  <div className="bg-[#111] border-2 border-white/10 p-8 rounded-none">
                    <h3 className="text-xl font-black text-white uppercase tracking-tighter italic mb-1">CLASIFICACIÓN GENERAL DEL HUB</h3>
                    <p className="text-xs text-neutral-400 uppercase tracking-tight font-mono">Listado de posiciones basado en el ELO real sincronizado de Faceit.</p>
                  </div>

                  {/* Leaderboard Table */}
                  <div className="bg-[#111] border-2 border-white/10 overflow-hidden shadow-[10px_10px_0px_rgba(0,0,0,0.5)] rounded-none">
                    <table className="w-full text-left border-collapse">
                      <thead>
                        <tr className="bg-neutral-900 border-b border-white/10">
                          <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400 w-20">Pos</th>
                          <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Discord ID</th>
                          <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Nickname Faceit</th>
                          <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Nivel</th>
                          <th className="py-4 px-6 text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">ELO</th>
                        </tr>
                      </thead>
                      <tbody className="divide-y divide-white/5 text-xs font-mono">
                        {leaderboard.map((u, i) => (
  <tr key={u.id} className="hover:bg-white/[0.02] border-b border-white/5 transition-colors">
    <td className="py-4 px-6 font-mono font-black text-xs text-white">
      {i === 0 ? <span className="text-yellow-500">01</span> : 
       i === 1 ? <span className="text-neutral-400">02</span> :
       i === 2 ? <span className="text-red-400">03</span> :
       String(i + 1).padStart(2, '0')}
    </td>
    <td className="py-4 px-6 text-white truncate max-w-[150px]">{u.discord_id || 'N/A'}</td>
    <td className="py-4 px-6 truncate max-w-[150px]">
      <button
        onClick={() => {
          setStatsSearch(u.faceit_nickname);
          setActiveTab('stats');
          setTimeout(() => handlePlayerSearch(undefined, u.faceit_nickname), 100);
        }}
        className="text-[#FF5500] hover:underline font-black text-left cursor-pointer focus:outline-none"
      >
        {u.faceit_nickname}
      </button>
    </td>
    <td className="py-4 px-6">
      <div className="flex items-center gap-2">
        <span className={`w-2 h-2 rounded-full ${u.faceit_level >= 10 ? 'bg-red-500' : u.faceit_level >= 8 ? 'bg-yellow-500' : 'bg-neutral-500'}`}></span>
        <span className="text-white">LVL {u.faceit_level}</span>
        {u.faceit_level_season && (
          <span className="text-[10px] font-mono font-bold px-1.5 py-0.5 bg-emerald-500/20 text-emerald-400 border border-emerald-500/30" title={`Actualizado en Season ${u.faceit_level_season}`}>
            S{u.faceit_level_season}
          </span>
        )}
      </div>
    </td>
    <td className="py-4 px-6 font-bold text-white">{u.faceit_elo}</td>
  </tr>
))}
                      </tbody>
                    </table>
                  </div>

                </div>
              )}

                           {/* VIEW 5: STATS ANALYSIS */}
              {activeTab === 'stats' && (
                <div className="space-y-8 animate-fade-in max-w-4xl mx-auto">
                  
                  {/* Search Title */}
                  <div className="text-center space-y-3">
                    <span className="text-xs font-mono font-black text-white bg-[#1A1A1A] border border-white/10 px-3 py-1 uppercase tracking-widest inline-block">ENGINE // FACEIT_DATA_STREAM</span>
                    <h3 className="text-3xl font-black tracking-tighter text-white uppercase italic">ANÁLISIS DE ESTADÍSTICAS</h3>
                    <p className="text-xs text-neutral-400 max-w-md mx-auto uppercase tracking-tight font-mono">Ingresa un nickname de Faceit para consultar sus datos en vivo de la base de datos oficial.</p>
                  </div>

                  {/* Search Input Bar */}
                  <form onSubmit={handlePlayerSearch} className="flex gap-2 max-w-md mx-auto">
                    <div className="relative flex-1">
                      <Search className="absolute left-4 top-1/2 -translate-y-1/2 text-neutral-400 w-4 h-4" />
                      <input
                        type="text"
                        placeholder="EJ: S1MPLE, ZYWOO, M0NESY..."
                        value={statsSearch}
                        onChange={(e) => setStatsSearch(e.target.value)}
                        className="w-full bg-[#111] border-2 border-white/10 text-xs font-mono font-bold text-white placeholder-neutral-500 rounded-none pl-12 pr-4 py-4 outline-none focus:border-white transition-all uppercase tracking-wider"
                      />
                    </div>
                    <button
                      type="submit"
                      className="px-6 py-4 bg-white hover:bg-neutral-200 text-black text-xs font-black uppercase tracking-tighter rounded-none cursor-pointer transition-colors shrink-0 border border-white"
                    >
                      Consultar
                    </button>
                  </form>

                  {/* Live Stats Display Card */}
                  {activePlayer && (
                  <div className="bg-[#111] border-2 border-white/10 p-8 space-y-8 relative overflow-hidden shadow-[10px_10px_0px_rgba(0,0,0,0.5)] rounded-none">
                    
                    {/* Identity Header */}
                    <div className="flex flex-col sm:flex-row items-center gap-6 relative z-10">
                      <img
                        src={activePlayer.avatarUrl}
                        alt={activePlayer.nickname}
                        className="w-24 h-24 rounded-none border-2 border-white object-cover bg-neutral-900"
                      />
                      <div className="text-center sm:text-left space-y-2">
                        <div className="flex flex-wrap items-center justify-center sm:justify-start gap-3">
                          <h4 className="text-2xl font-black text-white uppercase tracking-tight italic">{activePlayer.nickname}</h4>
                          <span className="text-[10px] bg-[#1C1C1C] px-2.5 py-1 rounded-none border border-white/10 text-white font-mono font-black uppercase tracking-widest">
                            🌍 {activePlayer.country}
                          </span>
                        </div>

                        <div className="flex items-center gap-2.5 justify-center sm:justify-start">
                          <span className="bg-white text-black px-3 py-0.5 rounded-none text-[9px] font-mono font-black border border-white uppercase tracking-wider">
                            {activePlayer.role}
                          </span>
                          <span className="bg-[#1C1C1C] border border-white/10 text-[9px] text-neutral-400 font-mono font-black px-2.5 py-0.5 rounded-none uppercase">
                            {activePlayer.membership}
                          </span>
                        </div>
                      </div>
                    </div>

                    <div className="h-px bg-white/10" />

                    {/* Bento Statistics Breakdown Grid */}
                    <div className="grid grid-cols-1 sm:grid-cols-2 md:grid-cols-4 gap-6">
                      
                      <div className="bg-neutral-900 border border-white/5 rounded-none p-5 relative overflow-hidden">
                        <span className="text-[9px] font-mono text-neutral-500 uppercase tracking-widest block mb-1 font-black">ELO ACTUAL</span>
                        <div className="text-3xl font-mono font-black text-white mb-1">{Number(activePlayer.elo || 0).toLocaleString('es-ES')}</div>
                        <span className="text-[9px] text-white font-black font-mono uppercase tracking-tight">
                          +15 (Última Partida)
                        </span>
                      </div>

                      <div className="bg-neutral-900 border border-white/5 rounded-none p-5 relative overflow-hidden">
                        <span className="text-[9px] font-mono text-neutral-500 uppercase tracking-widest block mb-1 font-black">TASA DE VICTORIA</span>
                        <div className="text-3xl font-mono font-black text-white mb-1">{activePlayer.winRate}%</div>
                        <div className="w-full bg-white/5 rounded-none h-1 overflow-hidden">
                          <div className="bg-white h-full" style={{ width: `${activePlayer.winRate}%` }} />
                        </div>
                      </div>

                      <div className="bg-neutral-900 border border-white/5 rounded-none p-5 relative overflow-hidden">
                        <span className="text-[9px] font-mono text-neutral-500 uppercase tracking-widest block mb-1 font-black">PROMEDIO K/D</span>
                        <div className="text-3xl font-mono font-black text-white mb-1">{activePlayer.kdRatio}</div>
                        <span className="text-[9px] text-neutral-400 font-black uppercase font-mono">Top 1% Global</span>
                      </div>

                      <div className="bg-neutral-900 border border-white/5 rounded-none p-5 relative overflow-hidden">
                        <span className="text-[9px] font-mono text-neutral-500 uppercase tracking-widest block mb-1 font-black">PARTIDAS JUGADAS</span>
                        <div className="text-3xl font-mono font-black text-white mb-1">{Number(activePlayer.totalMatches || 0).toLocaleString('es-ES')}</div>
                        <span className="text-[9px] text-neutral-400 font-black uppercase font-mono">Historial verificado</span>
                      </div>

                    </div>

                    {/* Regional / Global ranks */}
                    <div className="grid grid-cols-1 sm:grid-cols-2 gap-4">
                      <div className="flex items-center justify-between p-4 bg-neutral-900 border border-white/5 rounded-none font-mono">
                        <span className="text-[10px] text-neutral-400 font-black uppercase tracking-wider">RANGO GLOBAL</span>
                        <span className="text-xs font-black text-white">{activePlayer.globalRank}</span>
                      </div>
                      <div className="flex items-center justify-between p-4 bg-neutral-900 border border-white/5 rounded-none font-mono">
                        <span className="text-[10px] text-neutral-400 font-black uppercase tracking-wider">RANGO PAÍS</span>
                        <span className="text-xs font-black text-white">{activePlayer.countryRank}</span>
                      </div>
                    </div>

                    {/* Recent Match History Tracker (10 Dots Loop) */}
                    <div className="bg-neutral-900 border border-white/5 rounded-none p-6 space-y-4">
                      <h5 className="text-[10px] font-mono font-black uppercase tracking-wider text-neutral-400">HISTORIAL RECIENTE (ÚLTIMAS 10 PARTIDAS)</h5>
                      
                      <div className="flex flex-wrap items-center gap-4 justify-between">
                        <div className="flex items-center gap-3 overflow-x-auto pb-1">
                          {(activePlayer.recentForm || []).map((result: string, index: number) => {
                            const isWin = result === 'W';
                            const points = (activePlayer.recentPoints && activePlayer.recentPoints[index]) ? activePlayer.recentPoints[index] : 25;
                            return (
                              <div key={index} className="flex flex-col items-center gap-1.5 min-w-[36px]">
                                <div className={`w-8 h-8 rounded-none flex items-center justify-center font-black text-xs border ${
                                  isWin ? 'bg-white text-black border-white' : 'bg-[#0A0A0A] text-neutral-500 border-white/10'
                                }`}>
                                  {result}
                                </div>
                                <span className={`text-[9px] font-mono font-black ${isWin ? 'text-white' : 'text-neutral-500'}`}>
                                  {points > 0 ? `+${points}` : points}
                                </span>
                              </div>
                            );
                          })}
                        </div>
                        <div className="text-right font-mono">
                          <span className="text-[9px] text-neutral-500 font-black uppercase">PROMEDIO ELO</span>
                          <p className="text-xs font-black text-white uppercase">+{Math.ceil(((activePlayer.recentPoints || []).reduce((a: number, b: number) => a + b, 0)) / 10 || 25)} POR PARTIDA</p>
                        </div>
                      </div>
                    </div>

                  </div>
                  )}
                </div>
              )}

                     {/* VIEW 6: CONFIG PORTAL */}
              {activeTab === 'config' && (
                <div className="space-y-6 animate-fade-in max-w-6xl xl:max-w-7xl 2xl:max-w-[1550px] mx-auto">
                  
                  <div className="bg-[#111] border-2 border-white/10 p-8 rounded-none flex items-center justify-between">
                    <div>
                      <h3 className="text-xl font-black text-white uppercase tracking-tighter italic mb-1">CONFIGURACIÓN MAESTRA DEL BOT</h3>
                      <p className="text-xs text-neutral-400 uppercase tracking-tight font-mono">Los valores guardados aquí modifican el archivo .env del servidor en tiempo real.</p>
                    </div>
                  </div>

                  {currentUser?.role === 'global_admin' && (
                    <section className="bg-[#111] border-2 border-[#66C0F4]/30 p-6 rounded-none shadow-[8px_8px_0px_rgba(102,192,244,0.12)]">
                      <div className="flex flex-col gap-1 mb-5">
                        <h4 className="text-sm font-black text-[#66C0F4] uppercase tracking-tight">Steam Guard</h4>
                        <p className="text-xs text-neutral-400 font-mono">
                          Solicita el inicio una vez. Steam enviará el código al correo o lo pedirá desde el autenticador; el panel mostrará el estado real de la validación.
                        </p>
                      </div>
                      <div className={`mb-4 p-3 border text-xs font-mono transition-colors ${
                        steamLoginStatus.connected
                          ? 'bg-emerald-500/10 border-emerald-500/40 text-emerald-300'
                          : steamLoginStatus.status === 'rate_limited' || steamLoginStatus.status === 'error'
                            ? 'bg-red-500/10 border-red-500/40 text-red-300'
                            : steamLoginStatus.lastCodeWrong
                              ? 'bg-amber-500/15 border-amber-500/60 text-amber-200'
                              : 'bg-[#0A0A0A] border-white/10 text-neutral-300'
                      }`}>
                        <div className="flex flex-wrap items-center justify-between gap-2">
                          <span className="font-black uppercase tracking-wide">
                            {steamLoginStatus.connected
                              ? '● Conectado a Steam'
                              : steamLoginStatus.waitingForCode
                                ? steamLoginStatus.lastCodeWrong
                                  ? '● Código rechazado — Esperando código nuevo'
                                  : `● Esperando código de ${steamLoginStatus.guardType === 'email' ? 'correo' : 'autenticador'}`
                                : steamLoginBusy
                                  ? '● Validando conexión con Steam...'
                                : '● Desconectado'}
                          </span>
                          {steamLoginStatus.codeAttempts > 0 && (
                            <span className={`text-[10px] font-bold ${steamLoginStatus.lastCodeWrong ? 'text-amber-400' : 'opacity-80'}`}>
                              Intentos en esta sesión: {steamLoginStatus.codeAttempts}
                            </span>
                          )}
                        </div>
                        <p className="mt-1 leading-relaxed font-semibold">{steamLoginStatus.message}</p>
                      </div>
                      <div className="flex flex-wrap items-center gap-2 mb-3">
                        <button
                          type="button"
                          onClick={handleRequestSteamGuardCode}
                          disabled={requestingSteamGuardCode || steamLoginStatus.connected || steamLoginStatus.waitingForCode || steamLoginBusy || steamLoginStatus.retryAfterMs > 0}
                          className="px-5 py-3 bg-[#1B4F72] hover:bg-[#236B99] disabled:opacity-50 text-white text-xs font-black uppercase tracking-tight rounded-none transition-colors cursor-pointer"
                        >
                          {requestingSteamGuardCode ? 'Solicitando...' : steamLoginStatus.connected ? 'Bot conectado' : 'Solicitar código de Steam'}
                        </button>
                        {(steamLoginStatus.retryAfterMs > 0 || steamLoginStatus.codeAttempts > 0 || steamLoginStatus.status === 'error') && !steamLoginStatus.connected && (
                          <button
                            type="button"
                            onClick={handleResetSteamCooldown}
                            disabled={resettingSteamCooldown}
                            className="px-4 py-3 bg-red-950/40 hover:bg-red-900/60 text-red-300 border border-red-500/30 text-xs font-black uppercase tracking-tight rounded-none transition-colors cursor-pointer"
                          >
                            {resettingSteamCooldown ? 'Reiniciando...' : 'Reiniciar Sesión Steam'}
                          </button>
                        )}
                      </div>
                      <form onSubmit={handleSubmitSteamGuardCode} className="flex flex-col sm:flex-row gap-3">
                        <input
                          type="text"
                          value={steamGuardCode}
                          onChange={(event) => setSteamGuardCode(event.target.value.toUpperCase().replace(/[^A-Z0-9]/g, '').slice(0, 5))}
                          placeholder="ABCDE"
                          inputMode="text"
                          autoComplete="one-time-code"
                          maxLength={5}
                          aria-label="Código de Steam Guard"
                          disabled={!steamLoginStatus.waitingForCode || steamLoginStatus.retryAfterMs > 0 || sendingSteamGuardCode || steamLoginStatus.status === 'verifying'}
                          className="min-w-0 flex-1 bg-[#0A0A0A] border-2 border-white/10 text-sm font-mono font-black tracking-[0.35em] text-white uppercase rounded-none px-4 py-3 outline-none focus:border-[#66C0F4] transition-colors"
                        />
                        <button
                          type="submit"
                          disabled={sendingSteamGuardCode || steamLoginStatus.status === 'verifying' || !steamLoginStatus.waitingForCode || steamLoginStatus.retryAfterMs > 0}
                          className="px-5 py-3 bg-[#66C0F4] hover:bg-[#8DD5FF] disabled:opacity-50 text-black text-xs font-black uppercase tracking-tight rounded-none transition-colors cursor-pointer"
                        >
                          {sendingSteamGuardCode || steamLoginStatus.status === 'verifying' ? 'Validando...' : 'Enviar a Steam'}
                        </button>
                      </form>
                    </section>
                  )}

                  <form onSubmit={handleSaveConfig} className="space-y-8">
                    <datalist id="available-discord-roles-list">
                      {availableDiscordRoles.map((r) => (
                        <option key={r.id} value={r.id}>
                          @{r.name} ({r.id})
                        </option>
                      ))}
                    </datalist>
                    
                    {/* Canales de Discord */}
                    <div className="bg-[#111] border-2 border-[#5865F2]/20 p-8 shadow-[8px_8px_0px_rgba(88,101,242,0.15)] rounded-none relative">
                      <div className="absolute -top-3 -left-3 bg-[#5865F2] text-white text-[10px] font-black px-3 py-1 uppercase tracking-widest border border-white/10">Discord</div>
                      <div className="grid grid-cols-1 md:grid-cols-2 gap-6 mt-2">
                        <div className="flex flex-col gap-1.5 md:col-span-2">
                          <label className="text-[10px] font-mono font-black uppercase tracking-widest text-[#5865F2]/80">ID del Servidor de Discord (Guild ID)</label>
                          <input type="text" value={config.discordGuildId || ''} onChange={(e) => setConfig({ ...config, discordGuildId: e.target.value })} className="bg-[#0A0A0A] border-2 border-white/10 text-xs font-mono font-black text-white rounded-none px-4 py-3.5 outline-none focus:border-[#5865F2]/50 transition-colors" />
                        </div>
                        <div className="flex flex-col gap-1.5">
                          <label className="text-[10px] font-mono font-black uppercase tracking-widest text-[#5865F2]/80">ID Canal de Vinculación</label>
                          <ChannelSelect channels={discordChannels} kind="text" value={channelIdsOf(config, 'discordLinkChannelId')} onChange={(ids) => setConfig({ ...config, discordLinkChannelId: ids[0] || '' })} />
                        </div>
                        <div className="flex flex-col gap-1.5">
                          <label className="text-[10px] font-mono font-black uppercase tracking-widest text-[#5865F2]/80">ID Canal Anuncios de Partidas (No Limpia Historial)</label>
                          <ChannelSelect channels={discordChannels} kind="text" multiple value={channelIdsOf(config, 'discordMatchesChannelId')} onChange={(ids) => setConfig({ ...config, discordMatchesChannelId: ids[0] || '', discordMatchesChannelIds: ids })} />
                        </div>
                        <div className="flex flex-col gap-1.5">
                          <label className="text-[10px] font-mono font-black uppercase tracking-widest text-[#5865F2]/80">ID Canal Botón Mixton / Panel (Limpia Historial)</label>
                          <ChannelSelect channels={discordChannels} kind="text" value={channelIdsOf(config, 'discordMixtonChannelId')} onChange={(ids) => setConfig({ ...config, discordMixtonChannelId: ids[0] || '' })} />
                        </div>
                        <div className="flex flex-col gap-1.5">
                          <label className="text-[10px] font-mono font-black uppercase tracking-widest text-[#FF5500]/90">ID Canal Chat Mixtón & Torneo (Sincronizado con Web)</label>
                          <ChannelSelect channels={discordChannels} kind="text" triggerClassName='bg-[#0A0A0A] border-2 border-[#FF5500]/30 focus:border-[#FF5500]' value={channelIdsOf(config, 'discordMixtonChatChannelId')} onChange={(ids) => setConfig({ ...config, discordMixtonChatChannelId: ids[0] || '' })} />
                        </div>
                        <div className="flex flex-col gap-1.5">
                          <label className="text-[10px] font-mono font-black uppercase tracking-widest text-[#5865F2]/80">ID Canal Sincronización de Roles</label>
                          <ChannelSelect channels={discordChannels} kind="text" value={channelIdsOf(config, 'discordRolesChannelId')} onChange={(ids) => setConfig({ ...config, discordRolesChannelId: ids[0] || '' })} />
                        </div>
                        <div className="flex flex-col gap-1.5">
                          <label className="text-[10px] font-mono font-black uppercase tracking-widest text-[#5865F2]/80">ID Canal Historial de Logs</label>
                          <ChannelSelect channels={discordChannels} kind="text" multiple value={channelIdsOf(config, 'discordStatusChannelId')} onChange={(ids) => setConfig({ ...config, discordStatusChannelId: ids[0] || '', discordStatusChannelIds: ids })} />
                        </div>
                        <div className="flex flex-col gap-1.5">
                          <label className="text-[10px] font-mono font-black uppercase tracking-widest text-[#5865F2]/80">Sala de Espera (Voz)</label>
                          <ChannelSelect channels={discordChannels} kind="voice" value={channelIdsOf(config, 'discordWaitingRoomChannelId')} onChange={(ids) => setConfig({ ...config, discordWaitingRoomChannelId: ids[0] || '' })} />
                        </div>
                        <div className="flex flex-col gap-1.5">
                          <label className="text-[10px] font-mono font-black uppercase tracking-widest text-[#5865F2]/80">ID Canal de Alertas de Admin</label>
                          <ChannelSelect channels={discordChannels} kind="text" multiple value={channelIdsOf(config, 'discordAdminAlertsChannelId')} onChange={(ids) => setConfig({ ...config, discordAdminAlertsChannelId: ids[0] || '', discordAdminAlertsChannelIds: ids })} />
                        </div>
                        <div className="flex flex-col gap-1.5">
                          <label className="text-[10px] font-mono font-black uppercase tracking-widest text-[#5865F2]/80">ID Canal Alertas de Sanciones (Baneos)</label>
                          <ChannelSelect channels={discordChannels} kind="text" multiple value={channelIdsOf(config, 'discordSanctionsChannelId')} onChange={(ids) => setConfig({ ...config, discordSanctionsChannelId: ids[0] || '', discordSanctionsChannelIds: ids })} />
                        </div>
                        <div className="flex flex-col gap-1.5 md:col-span-2">
                          <label className="text-[10px] font-mono font-black uppercase tracking-widest text-[#5865F2]/80">ID Canal de Soporte Público (Autodespliegue)</label>
                          <ChannelSelect channels={discordChannels} kind="text" multiple value={channelIdsOf(config, 'discordSupportChannelId')} onChange={(ids) => setConfig({ ...config, discordSupportChannelId: ids[0] || '', discordSupportChannelIds: ids })} />
                        </div>
                        <div className="flex flex-col gap-1.5 md:col-span-2">
                          <label className="text-[10px] font-mono font-black uppercase tracking-widest text-[#5865F2]/80">ID Canal de Música (/cs2_play)</label>
                          <ChannelSelect channels={discordChannels} value={channelIdsOf(config, 'discordMusicChannelId')} onChange={(ids) => setConfig({ ...config, discordMusicChannelId: ids[0] || '' })} />
                        </div>
                        <div className="flex flex-col gap-1.5 md:col-span-2">
                          <label className="text-[10px] font-mono font-black uppercase tracking-widest text-[#5865F2]/80">ID Canal de Recargas de Saldo (Botón Yape / Panel Autodespliegue)</label>
                          <ChannelSelect channels={discordChannels} kind="text" value={channelIdsOf(config, 'discordRechargeChannelId')} onChange={(ids) => setConfig({ ...config, discordRechargeChannelId: ids[0] || '' })} />
                        </div>
                        <div className="flex flex-col gap-1.5 md:col-span-2">
                          <label className="text-[10px] font-mono font-black uppercase tracking-widest text-[#5865F2]/80">ID Rol Acceso a Salas de Partida (Espectador / Staff)</label>
                          <input type="text" value={config.discordMatchAccessRoleId || ''} onChange={(e) => setConfig({ ...config, discordMatchAccessRoleId: e.target.value })} className="bg-[#0A0A0A] border-2 border-white/10 text-xs font-mono font-black text-white rounded-none px-4 py-3.5 outline-none focus:border-[#5865F2]/50 transition-colors" placeholder="Ej: 123456789012345678" />
                        </div>
                        <div className="flex flex-col gap-1.5 md:col-span-2">
                          <label className="text-[10px] font-mono font-black uppercase tracking-widest text-[#5865F2] flex items-center justify-between">
                            <span>ID Rol de Discord para Season (1ra Partida General - Fallback)</span>
                            <span className="text-[9px] text-emerald-400 font-bold normal-case">Rol en tu Servidor de Discord</span>
                          </label>
                          <input
                            type="text"
                            list="available-discord-roles-list"
                            value={config.discordHubRoleId || ''}
                            onChange={(e) => setConfig({ ...config, discordHubRoleId: e.target.value })}
                            className="bg-[#0A0A0A] border-2 border-[#5865F2]/40 text-xs font-mono font-black text-white rounded-none px-4 py-3.5 outline-none focus:border-[#5865F2] transition-colors"
                            placeholder="Pega el ID del rol de Discord o escribe para buscar"
                          />
                          <span className="text-[10px] font-mono text-neutral-400">
                            {availableDiscordRoles.find(r => r.id === config.discordHubRoleId)
                              ? `✓ Rol detectado: @${availableDiscordRoles.find(r => r.id === config.discordHubRoleId)?.name}`
                              : 'ID del rol de Discord que se otorga al jugar su primera partida de la season en cualquier cola (si la cola no tiene rol específico).'}
                          </span>
                        </div>
                      </div>
                    </div>

                    {/* Integración Faceit */}
                    <div className="bg-[#111] border-2 border-[#FF5500]/20 p-8 shadow-[8px_8px_0px_rgba(255,85,0,0.15)] rounded-none relative">
                      <div className="absolute -top-3 -left-3 bg-[#FF5500] text-white text-[10px] font-black px-3 py-1 uppercase tracking-widest border border-white/10">Faceit</div>
                      <div className="grid grid-cols-1 md:grid-cols-2 gap-6 mt-2">
                        <div className="flex flex-col gap-1.5">
                          <label className="text-[10px] font-mono font-black uppercase tracking-widest text-[#FF5500]/80">ID Hub Oficial Faceit</label>
                          <input type="text" value={config.faceitHubId} onChange={(e) => setConfig({ ...config, faceitHubId: e.target.value })} className="bg-[#0A0A0A] border-2 border-white/10 text-xs font-mono font-black text-white rounded-none px-4 py-3.5 outline-none focus:border-[#FF5500]/50 transition-colors" />
                        </div>
                        <div className="flex flex-col gap-1.5">
                          <label className="text-[10px] font-mono font-black uppercase tracking-widest text-[#FF5500]/80">ID Cola / Leaderboard</label>
                          <input type="text" value={config.faceitLeaderboardId || ''} onChange={(e) => setConfig({ ...config, faceitLeaderboardId: e.target.value })} className="bg-[#0A0A0A] border-2 border-white/10 text-xs font-mono font-black text-white rounded-none px-4 py-3.5 outline-none focus:border-[#FF5500]/50 transition-colors" />
                        </div>
                        <div className="flex flex-col gap-1.5 md:col-span-2">
                          <label className="text-[10px] font-mono font-black uppercase tracking-widest text-[#FF5500]/80">ID Rol del Hub Faceit (Default / Fallback)</label>
                          <input type="text" value={config.hubRoleId || ''} onChange={(e) => setConfig({ ...config, hubRoleId: e.target.value })} className="bg-[#0A0A0A] border-2 border-white/10 text-xs font-mono font-black text-white rounded-none px-4 py-3.5 outline-none focus:border-[#FF5500]/50 transition-colors" placeholder="ID o nombre del rol en el Hub de Faceit" />
                        </div>
                      </div>
                    </div>

                    {/* Multi-Colas del Hub */}
                    <div className="bg-[#111] border-2 border-emerald-500/30 p-8 shadow-[8px_8px_0px_rgba(16,185,129,0.15)] rounded-none relative space-y-6">
                      <div className="absolute -top-3 -left-3 bg-emerald-500 text-black text-[10px] font-black px-3 py-1 uppercase tracking-widest border border-white/10">
                        Multi-Cola del Hub (Paralelas)
                      </div>

                      <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3 pt-1">
                        <p className="text-xs text-neutral-400 font-mono">
                          Configura las colas independientes del Hub (5v5 Principal, 5v5 Secundaria, etc.) con sus respectivos canales de Discord.
                        </p>
                        <div className="flex items-center gap-2 shrink-0">
                          <button
                            type="button"
                            onClick={handleSyncFaceitQueues}
                            disabled={isSyncingFaceitQueues}
                            className="px-3.5 py-2 bg-[#FF5500] hover:bg-[#FF7700] text-white text-[11px] font-black uppercase tracking-wider rounded-none transition-colors flex items-center gap-1.5 shadow-[2px_2px_0px_rgba(0,0,0,0.5)] cursor-pointer disabled:opacity-50"
                          >
                            <RefreshCw className={`w-3.5 h-3.5 ${isSyncingFaceitQueues ? 'animate-spin' : ''}`} />
                            {isSyncingFaceitQueues ? 'SINCRONIZANDO...' : 'SINCRONIZAR DESDE FACEIT'}
                          </button>
                          <button
                            type="button"
                            onClick={handleAddQueue}
                            className="px-4 py-2 bg-emerald-500 hover:bg-emerald-400 text-black text-[11px] font-black uppercase tracking-wider rounded-none transition-colors flex items-center gap-1.5 shadow-[2px_2px_0px_rgba(0,0,0,0.5)] cursor-pointer"
                          >
                            <Plus className="w-3.5 h-3.5" />
                            AÑADIR COLA
                          </button>
                        </div>
                      </div>

                      {(!config.queues || config.queues.length === 0) ? (
                        <div className="bg-[#0A0A0A] border border-dashed border-white/15 p-6 text-center text-xs text-neutral-500 font-mono">
                          No hay colas secundarias configuradas. Si dejas esto vacío, el bot usará los canales de Discord generales configurados arriba.
                        </div>
                      ) : (
                        <div className="space-y-6">
                          {config.queues.map((q, idx) => (
                            <div key={q.id || idx} className="bg-[#0A0A0A] border-2 border-white/10 p-5 relative space-y-4">
                              <div className="flex items-center justify-between border-b border-white/10 pb-3">
                                <span className="text-xs font-black uppercase tracking-wider text-emerald-400 font-mono">
                                  🎯 COLA #{idx + 1}: {q.name || 'Sin Nombre'}
                                </span>
                                <button
                                  type="button"
                                  onClick={() => handleRemoveQueue(idx)}
                                  className="text-xs text-red-400 hover:text-red-300 flex items-center gap-1 font-mono uppercase tracking-wider transition-colors cursor-pointer"
                                >
                                  <Trash2 className="w-3.5 h-3.5" /> Eliminar
                                </button>
                              </div>

                              <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
                                <div className="flex flex-col gap-1">
                                  <label className="text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Nombre de la Cola (Discord)</label>
                                  <input
                                    type="text"
                                    value={q.name || ''}
                                    placeholder="Ej: 5v5 Principal"
                                    onChange={(e) => handleUpdateQueue(idx, 'name', e.target.value)}
                                    className="bg-[#111] border border-white/15 text-xs font-mono text-white px-3 py-2 outline-none focus:border-emerald-500 transition-colors"
                                  />
                                </div>
                                <div className="flex flex-col gap-1">
                                  <label className="text-[10px] font-mono font-black uppercase tracking-widest text-emerald-400/80">ID de la Cola FACEIT (queue_id)</label>
                                  <input
                                    type="text"
                                    value={q.queueId || q.id || ''}
                                    placeholder="UUID de la cola en Faceit"
                                    onChange={(e) => handleUpdateQueue(idx, 'queueId', e.target.value)}
                                    className="bg-[#111] border border-white/15 text-xs font-mono text-white px-3 py-2 outline-none focus:border-emerald-500 transition-colors"
                                  />
                                </div>
                                <div className="flex flex-col gap-1">
                                  <label className="text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">ID Canal Anuncios de Partidas (Texto)</label>
                                  <ChannelSelect channels={discordChannels} kind="text" triggerClassName='bg-[#111] border border-white/15 focus:border-emerald-500' value={channelIdsOf(q, 'discordMatchesChannelId')} onChange={(ids) => handleUpdateQueue(idx, 'discordMatchesChannelId', ids[0] || '')} />
                                </div>
                                <div className="flex flex-col gap-1">
                                  <label className="text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">ID Sala de Espera de Voz (Voz)</label>
                                  <ChannelSelect channels={discordChannels} kind="voice" triggerClassName='bg-[#111] border border-white/15 focus:border-emerald-500' value={channelIdsOf(q, 'discordWaitingRoomChannelId')} onChange={(ids) => handleUpdateQueue(idx, 'discordWaitingRoomChannelId', ids[0] || '')} />
                                </div>
                                <div className="flex flex-col gap-1">
                                  <label className="text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">ID Canal Panel Mixton (Opcional)</label>
                                  <ChannelSelect channels={discordChannels} kind="text" triggerClassName='bg-[#111] border border-white/15 focus:border-emerald-500' value={channelIdsOf(q, 'discordMixtonChannelId')} onChange={(ids) => handleUpdateQueue(idx, 'discordMixtonChannelId', ids[0] || '')} />
                                </div>
                                <div className="flex flex-col gap-1">
                                  <label className="text-[10px] font-mono font-black uppercase tracking-widest text-[#FF5500]">ID Canal Chat Sala / Torneo (Opcional)</label>
                                  <ChannelSelect channels={discordChannels} kind="text" triggerClassName='bg-[#111] border border-[#FF5500]/30 focus:border-[#FF5500]' value={channelIdsOf(q, 'discordMixtonChatChannelId')} onChange={(ids) => handleUpdateQueue(idx, 'discordMixtonChatChannelId', ids[0] || '')} />
                                </div>
                                <div className="flex flex-col gap-1">
                                  <label className="text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">ID Rol Acceso Salas (Opcional)</label>
                                  <input
                                    type="text"
                                    value={q.discordMatchAccessRoleId || ''}
                                    placeholder="ID rol acceso a salas de esta cola"
                                    onChange={(e) => handleUpdateQueue(idx, 'discordMatchAccessRoleId', e.target.value)}
                                    className="bg-[#111] border border-white/15 text-xs font-mono text-white px-3 py-2 outline-none focus:border-emerald-500 transition-colors"
                                  />
                                </div>
                                <div className="flex flex-col gap-1 bg-[#141414] p-3 border border-[#FF5500]/40">
                                  <label className="text-[10px] font-mono font-black uppercase tracking-widest text-[#FF5500] flex items-center justify-between">
                                    <span className="flex items-center gap-1.5">
                                      <span className="w-2 h-2 rounded-full bg-[#FF5500] inline-block"></span>
                                      ID ROL DEL HUB (FACEIT)
                                    </span>
                                    <span className="text-[9px] text-[#FF5500] font-bold normal-case">Faceit Hub</span>
                                  </label>
                                  <input
                                    type="text"
                                    value={q.hubRoleId || ''}
                                    placeholder="ID o nombre del rol en Faceit Hub"
                                    onChange={(e) => handleUpdateQueue(idx, 'hubRoleId', e.target.value)}
                                    className="bg-[#111] border border-[#FF5500]/50 text-xs font-mono text-white px-3 py-2 outline-none focus:border-[#FF5500] transition-colors"
                                  />
                                  <span className="text-[10px] font-mono text-neutral-400 mt-0.5">
                                    ID o nombre del rol interno configurado en Faceit Hub para esta cola.
                                  </span>
                                </div>

                                <div className="flex flex-col gap-1 bg-[#141414] p-3 border border-[#5865F2]/50">
                                  <label className="text-[10px] font-mono font-black uppercase tracking-widest text-[#5865F2] flex items-center justify-between">
                                    <span className="flex items-center gap-1.5">
                                      <span className="w-2 h-2 rounded-full bg-[#5865F2] inline-block"></span>
                                      ID ROL DE DISCORD (SEASON / 1RA PARTIDA)
                                    </span>
                                    <span className="text-[9px] text-emerald-400 font-bold normal-case">Servidor Discord</span>
                                  </label>
                                  <input
                                    type="text"
                                    list="available-discord-roles-list"
                                    value={q.discordHubRoleId || ''}
                                    placeholder="Pega el ID del rol de Discord o selecciónalo"
                                    onChange={(e) => handleUpdateQueue(idx, 'discordHubRoleId', e.target.value)}
                                    className="bg-[#111] border border-[#5865F2]/60 text-xs font-mono text-white px-3 py-2 outline-none focus:border-[#5865F2] transition-colors"
                                  />
                                  <span className="text-[10px] font-mono text-neutral-400 mt-0.5">
                                    {availableDiscordRoles.find(r => r.id === q.discordHubRoleId) ? (
                                      <span className="text-emerald-400 font-bold">
                                        ✓ Rol detectado en Discord: @{availableDiscordRoles.find(r => r.id === q.discordHubRoleId)?.name}
                                      </span>
                                    ) : (
                                      'ID del rol de Discord asignado automáticamente al jugador en su 1ra partida de la Season.'
                                    )}
                                  </span>
                                </div>
                              </div>
                            </div>
                          ))}
                        </div>
                      )}
                    </div>

                    {/* Pool de Mapas CS2 para Veto Mixton */}
                    <div className="bg-[#111] border-2 border-[#FF5500]/30 p-8 shadow-[8px_8px_0px_rgba(255,85,0,0.12)] rounded-none relative">
                      <div className="absolute -top-3 -left-3 bg-[#FF5500] text-black text-[10px] font-black px-3 py-1 uppercase tracking-widest border border-black/20 flex items-center gap-1.5">
                        <Map className="w-3.5 h-3.5" /> Pool de Mapas CS2 (Veto Mixton)
                      </div>

                      <div className="flex flex-col lg:flex-row lg:items-center justify-between gap-4 mt-2 mb-6">
                        <div>
                          <h4 className="text-base font-black text-white uppercase tracking-tight flex items-center gap-2">
                            CONFIGURACIÓN DE MAPAS DISPONIBLES EN EL VETO
                            <span className="text-xs font-mono font-bold bg-[#FF5500]/20 text-[#FF5500] border border-[#FF5500]/40 px-2 py-0.5">
                              {mapCatalog.filter(m => m.active).length} ACTIVOS DE {mapCatalog.length}
                            </span>
                          </h4>
                          <p className="text-xs text-neutral-400 font-mono mt-1">
                            Selecciona qué mapas aparecerán en la fase interactiva de veto. El sistema calculará automáticamente la secuencia de baneos (1-2-2-1...) según la cantidad de mapas activos hasta dejar exactamente 1 mapa ganador.
                          </p>
                        </div>

                        {/* Presets rápidos */}
                        <div className="flex flex-wrap items-center gap-2">
                          <span className="text-[10px] font-mono text-neutral-500 uppercase font-black">Presets:</span>
                          <button
                            type="button"
                            onClick={() => handleApplyPreset('premier')}
                            className="px-2.5 py-1 bg-[#1A1A1A] hover:bg-[#252525] border border-white/20 text-[10px] font-mono font-bold text-white uppercase transition-colors"
                          >
                            🏆 Premier (7)
                          </button>
                          <button
                            type="button"
                            onClick={() => handleApplyPreset('all')}
                            className="px-2.5 py-1 bg-[#1A1A1A] hover:bg-[#252525] border border-white/20 text-[10px] font-mono font-bold text-white uppercase transition-colors"
                          >
                            🗺️ Todos (14)
                          </button>
                          <button
                            type="button"
                            onClick={() => handleApplyPreset('classics')}
                            className="px-2.5 py-1 bg-[#1A1A1A] hover:bg-[#252525] border border-white/20 text-[10px] font-mono font-bold text-white uppercase transition-colors"
                          >
                            ⚡ Clásicos (5)
                          </button>
                          <button
                            type="button"
                            onClick={() => handleApplyPreset('fast')}
                            className="px-2.5 py-1 bg-[#1A1A1A] hover:bg-[#252525] border border-white/20 text-[10px] font-mono font-bold text-white uppercase transition-colors"
                          >
                            🎯 Rápido (3)
                          </button>
                          <button
                            type="button"
                            onClick={() => setShowAddMapModal(true)}
                            className="px-2.5 py-1 bg-emerald-500/20 hover:bg-emerald-500/30 border border-emerald-500/40 text-[10px] font-mono font-bold text-emerald-300 uppercase transition-colors flex items-center gap-1"
                          >
                            <Plus className="w-3 h-3" /> Añadir Mapa
                          </button>
                        </div>
                      </div>

                      {/* Grid de Mapas */}
                      <div className="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-7 gap-3 sm:gap-4 mb-6">
                        {mapCatalog.map((mapItem) => {
                          const isActive = mapItem.active;
                          return (
                            <div
                              key={mapItem.id}
                              onClick={() => handleToggleMap(mapItem.id)}
                              className={`group relative flex flex-col justify-between h-52 sm:h-56 border-2 transition-all cursor-pointer overflow-hidden select-none ${
                                isActive
                                  ? 'border-emerald-500/70 shadow-[0_0_15px_rgba(16,185,129,0.2)] bg-[#141414]'
                                  : 'border-white/10 opacity-40 hover:opacity-75 bg-[#0A0A0A]'
                              }`}
                            >
                              {/* Imagen de fondo */}
                              <div
                                className="absolute inset-0 bg-cover bg-center transition-transform duration-500 group-hover:scale-105"
                                style={{ backgroundImage: `url(${mapItem.image})` }}
                              />
                              <div className={`absolute inset-0 transition-opacity ${isActive ? 'bg-gradient-to-t from-black/95 via-black/40 to-black/60' : 'bg-black/80'}`} />

                              {/* Badge superior */}
                              <div className="relative z-10 p-2 flex items-center justify-between">
                                <span className={`text-[8px] font-mono font-black uppercase px-1.5 py-0.5 border ${
                                  isActive ? 'bg-emerald-500 text-black border-emerald-400 font-bold' : 'bg-neutral-800 text-neutral-400 border-neutral-700'
                                }`}>
                                  {isActive ? '✓ ACTIVO' : '✕ INACTIVO'}
                                </span>
                                {mapItem.isCustom && (
                                  <button
                                    type="button"
                                    onClick={(e) => {
                                      e.stopPropagation();
                                      handleDeleteCustomMap(mapItem.id);
                                    }}
                                    className="p-1 bg-red-950/80 hover:bg-red-800 text-red-200 border border-red-500/40 transition-colors"
                                    title="Eliminar mapa personalizado"
                                  >
                                    <Trash2 className="w-2.5 h-2.5" />
                                  </button>
                                )}
                              </div>

                              {/* Icono central de insignia */}
                              <div className="relative z-10 flex items-center justify-center py-1">
                                <img
                                  src={mapItem.icon}
                                  alt={mapItem.name}
                                  className={`w-14 h-14 sm:w-16 sm:h-16 object-contain drop-shadow-[0_4px_8px_rgba(0,0,0,0.9)] transition-transform group-hover:scale-110 ${
                                    !isActive ? 'grayscale' : ''
                                  }`}
                                  onError={(e) => { (e.target as HTMLElement).style.display = 'none'; }}
                                />
                              </div>

                              {/* Nombre inferior */}
                              <div className="relative z-10 p-2 text-center bg-black/75 border-t border-white/10">
                                <span className={`text-xs font-black uppercase tracking-wider block truncate ${isActive ? 'text-white' : 'text-neutral-400'}`}>
                                  {mapItem.name}
                                </span>
                                <span className="text-[8px] font-mono text-neutral-500 block truncate">
                                  {mapItem.id}
                                </span>
                              </div>
                            </div>
                          );
                        })}
                      </div>

                      {/* Botón de Guardado de Mapas */}
                      <div className="pt-4 border-t border-white/10 flex flex-col sm:flex-row items-center justify-between gap-3">
                        <div className="text-[11px] font-mono text-neutral-400">
                          <span className="text-white font-bold">{mapCatalog.filter(m => m.active).length} mapas</span> seleccionados para el veto.
                          {mapCatalog.filter(m => m.active).length >= 2 && (
                            <span className="text-neutral-500 ml-1">
                              (Se realizarán {mapCatalog.filter(m => m.active).length - 1} baneos hasta quedar 1 mapa).
                            </span>
                          )}
                        </div>
                        <button
                          type="button"
                          onClick={handleSaveMapCatalog}
                          disabled={savingMapCatalog || loadingMapCatalog}
                          className="px-6 py-2.5 bg-[#FF5500] hover:bg-[#e04c00] text-black text-xs font-black uppercase tracking-tighter transition-colors cursor-pointer border border-[#FF5500] shadow-[3px_3px_0px_rgba(255,85,0,0.3)] disabled:opacity-40 flex items-center gap-2"
                        >
                          {savingMapCatalog ? <RefreshCw className="w-3.5 h-3.5 animate-spin" /> : <CheckCircle2 className="w-3.5 h-3.5" />}
                          {savingMapCatalog ? 'GUARDANDO MAPAS...' : '💾 GUARDAR POOL DE MAPAS'}
                        </button>
                      </div>
                    </div>

                    {/* Torneo & Mixtón CS2 — Rondas por Juego */}
                    <div className="bg-[#111] border-2 border-amber-500/40 p-8 shadow-[8px_8px_0px_rgba(245,158,11,0.15)] rounded-none relative">
                      <div className="absolute -top-3 -left-3 bg-amber-500 text-black text-[10px] font-black px-3 py-1 uppercase tracking-widest border border-black/20 flex items-center gap-1.5">
                        <Trophy className="w-3.5 h-3.5" /> Torneo & Mixtón CS2
                      </div>

                      <div className="flex flex-col lg:flex-row lg:items-center justify-between gap-4 mt-2 mb-5">
                        <div>
                          <h4 className="text-base font-black text-white uppercase tracking-tight flex items-center gap-2">
                            Juegos por Enfrentamiento
                            <span className="text-xs font-mono font-bold bg-amber-500/20 text-amber-400 border border-amber-500/40 px-2.5 py-0.5 tabular-nums">
                              {[1, 3, 5].includes(Number(config.gamesPerMatch ?? config.roundsPerGame)) ? Number(config.gamesPerMatch ?? config.roundsPerGame) : 1} {(Number(config.gamesPerMatch ?? config.roundsPerGame) || 1) === 1 ? 'JUEGO' : 'JUEGOS'}
                            </span>
                          </h4>
                          <p className="text-xs text-neutral-400 font-mono mt-1">
                            Cantidad de juegos (1, 3 o 5) para dar un ganador del enfrentamiento en Torneo y Mixtón. Por defecto es 1.
                          </p>
                        </div>
                      </div>

                      <div className="grid grid-cols-1 lg:grid-cols-12 gap-6 items-center bg-[#0A0A0A] border border-white/10 p-5">
                        <div className="lg:col-span-12 flex flex-col gap-3">
                          <label className="text-[10px] font-mono font-black uppercase tracking-widest text-amber-400">
                            Seleccionar Formato del Enfrentamiento
                          </label>
                          <div className="grid grid-cols-1 sm:grid-cols-3 gap-3">
                            {[
                              { games: 1, label: '1 Juego', sub: 'Al mejor de 1 (Gana 1 juego)' },
                              { games: 3, label: '3 Juegos', sub: 'Al mejor de 3 (Gana el primero con 2)' },
                              { games: 5, label: '5 Juegos', sub: 'Al mejor de 5 (Gana el primero con 3)' },
                            ].map((preset) => {
                              const currentVal = [1, 3, 5].includes(Number(config.gamesPerMatch ?? config.roundsPerGame)) ? Number(config.gamesPerMatch ?? config.roundsPerGame) : 1;
                              const active = currentVal === preset.games;
                              return (
                                <button
                                  key={preset.games}
                                  type="button"
                                  onClick={() => setConfig({ ...config, gamesPerMatch: preset.games, roundsPerGame: preset.games })}
                                  className={`p-4 text-left transition-all cursor-pointer border flex flex-col gap-1.5 ${
                                    active
                                      ? 'bg-amber-500 text-black border-amber-400 shadow-[2px_2px_0px_rgba(245,158,11,0.4)]'
                                      : 'bg-[#141414] hover:bg-[#1F1F1F] text-neutral-300 border-white/15'
                                  }`}
                                >
                                  <div className="flex items-center justify-between">
                                    <span className="text-sm font-mono font-black uppercase tabular-nums">
                                      {preset.label}
                                    </span>
                                    {active && (
                                      <span className="text-[10px] font-black uppercase px-2 py-0.5 bg-black text-amber-400">
                                        Activo
                                      </span>
                                    )}
                                  </div>
                                  <span className={`text-xs font-mono ${active ? 'text-black/80 font-bold' : 'text-neutral-400'}`}>
                                    {preset.sub}
                                  </span>
                                </button>
                              );
                            })}
                          </div>
                        </div>
                      </div>
                    </div>

                    {/* Sistema */}
                    <div className="bg-[#111] border-2 border-white/10 p-8 shadow-[8px_8px_0px_rgba(255,255,255,0.05)] rounded-none relative">
                      <div className="absolute -top-3 -left-3 bg-white text-black text-[10px] font-black px-3 py-1 uppercase tracking-widest border border-white/10">Sistema</div>
                      <div className="grid grid-cols-1 md:grid-cols-2 gap-6 mt-2">
                        <div className="flex flex-col gap-1.5">
                          <label className="text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">ID Rol Admin</label>
                          <input type="text" value={config.adminRoleId || ''} onChange={(e) => setConfig({ ...config, adminRoleId: e.target.value })} placeholder="Ej: 123456789012345678" className="bg-[#0A0A0A] border-2 border-white/10 text-xs font-mono font-black text-white rounded-none px-4 py-3.5 outline-none focus:border-white transition-colors" />
                        </div>
                        <div className="flex flex-col gap-1.5">
                          <label className="text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Timeout Partidas (Mins)</label>
                          <input type="number" value={config.matchTimeoutMinutes} onChange={(e) => setConfig({ ...config, matchTimeoutMinutes: parseInt(e.target.value, 10) || 0 })} className="bg-[#0A0A0A] border-2 border-white/10 text-xs font-mono font-black text-white rounded-none px-4 py-3.5 outline-none focus:border-white transition-colors" />
                        </div>
                      </div>

                      <div className="pt-8 mt-4 border-t border-white/10 flex justify-end">
                        <button type="submit" className="px-8 py-3 bg-white hover:bg-neutral-200 text-black text-xs font-black uppercase tracking-tighter rounded-none transition-colors cursor-pointer border border-white shadow-[4px_4px_0px_rgba(255,255,255,0.2)]">
                          💾 GUARDAR EN .ENV
                        </button>
                      </div>
                    </div>

                  </form>

                  {/* Modal de Añadir Mapa Personalizado */}
                  {showAddMapModal && (
                    <div className="fixed inset-0 z-50 bg-black/80 backdrop-blur-sm flex items-center justify-center p-4">
                      <div className="bg-[#111] border-2 border-white/20 p-6 max-w-md w-full shadow-[10px_10px_0px_rgba(0,0,0,0.7)]">
                        <div className="flex items-center justify-between pb-3 border-b border-white/10 mb-4">
                          <h4 className="text-sm font-black uppercase text-white tracking-wider flex items-center gap-2">
                            <Plus className="w-4 h-4 text-emerald-400" /> Añadir Mapa Personalizado
                          </h4>
                          <button
                            type="button"
                            onClick={() => setShowAddMapModal(false)}
                            className="text-neutral-400 hover:text-white"
                          >
                            <X className="w-4 h-4" />
                          </button>
                        </div>

                        <div className="space-y-3 font-mono text-xs">
                          <div>
                            <label className="text-[10px] uppercase text-neutral-400 block mb-1">ID del Mapa (ej: de_tuscan)</label>
                            <input
                              type="text"
                              value={newCustomMap.id}
                              onChange={(e) => setNewCustomMap({ ...newCustomMap, id: e.target.value })}
                              placeholder="de_tuscan"
                              className="w-full bg-[#0A0A0A] border border-white/20 px-3 py-2 text-white font-mono text-xs outline-none focus:border-white"
                            />
                          </div>
                          <div>
                            <label className="text-[10px] uppercase text-neutral-400 block mb-1">Nombre Visible (ej: Tuscan)</label>
                            <input
                              type="text"
                              value={newCustomMap.name}
                              onChange={(e) => setNewCustomMap({ ...newCustomMap, name: e.target.value })}
                              placeholder="Tuscan"
                              className="w-full bg-[#0A0A0A] border border-white/20 px-3 py-2 text-white font-mono text-xs outline-none focus:border-white"
                            />
                          </div>
                          <div>
                            <label className="text-[10px] uppercase text-neutral-400 block mb-1">URL Imagen Fondo (Opcional)</label>
                            <input
                              type="text"
                              value={newCustomMap.image}
                              onChange={(e) => setNewCustomMap({ ...newCustomMap, image: e.target.value })}
                              placeholder="https://.../map.png"
                              className="w-full bg-[#0A0A0A] border border-white/20 px-3 py-2 text-white font-mono text-xs outline-none focus:border-white"
                            />
                          </div>
                          <div>
                            <label className="text-[10px] uppercase text-neutral-400 block mb-1">URL Ícono / Insignia (Opcional)</label>
                            <input
                              type="text"
                              value={newCustomMap.icon}
                              onChange={(e) => setNewCustomMap({ ...newCustomMap, icon: e.target.value })}
                              placeholder="https://.../icon.png"
                              className="w-full bg-[#0A0A0A] border border-white/20 px-3 py-2 text-white font-mono text-xs outline-none focus:border-white"
                            />
                          </div>
                        </div>

                        <div className="mt-6 flex justify-end gap-2 font-mono">
                          <button
                            type="button"
                            onClick={() => setShowAddMapModal(false)}
                            className="px-4 py-2 border border-white/20 text-neutral-400 hover:text-white text-xs uppercase"
                          >
                            Cancelar
                          </button>
                          <button
                            type="button"
                            onClick={handleAddCustomMap}
                            className="px-4 py-2 bg-emerald-500 hover:bg-emerald-400 text-black font-black text-xs uppercase tracking-wider"
                          >
                            Añadir Mapa
                          </button>
                        </div>
                      </div>
                    </div>
                  )}
                </div>
              )}

              {/* VIEW 7: SERVICIOS */}
              {activeTab === 'features' && (
                <div className="space-y-6 animate-fade-in max-w-6xl mx-auto font-mono">
                  
                  {/* Encabezado Principal */}
                  <div className="bg-[#111] border-2 border-white/10 p-6 md:p-8 rounded-none shadow-[10px_10px_0px_rgba(0,0,0,0.5)]">
                    <div className="flex flex-col lg:flex-row lg:items-center justify-between gap-6">
                      <div className="space-y-2">
                        <div className="flex items-center gap-3">
                          <span className="text-[10px] font-black uppercase px-2.5 py-0.5 bg-white/10 text-neutral-300 border border-white/20 tracking-wider">
                            PANEL DE CONTROL DE SERVICIOS
                          </span>
                          <span className="flex items-center gap-1.5 text-[10px] font-black uppercase px-2.5 py-0.5 bg-emerald-500/20 text-emerald-400 border border-emerald-500/30 tracking-wider">
                            <span className="w-2 h-2 rounded-full bg-emerald-400 animate-pulse inline-block" />
                            {featureStats.active}/{featureStats.total} ACTIVOS ({featureStats.percent}%)
                          </span>
                        </div>
                        <h3 className="text-2xl md:text-3xl font-black text-white uppercase tracking-tighter italic">
                          FUNCIONALIDADES Y MÓDULOS DEL BOT
                        </h3>
                        <p className="text-xs text-neutral-400 uppercase tracking-tight">
                          Control maestro en tiempo real de los 32 servicios lógicos, bot de música, automatizaciones y comandos de Discord.
                        </p>
                        <div className="pt-2 flex flex-wrap items-center gap-2">
                          <span className="text-[10px] text-neutral-500 uppercase tracking-widest font-black">ÁMBITO ACTIVO:</span>
                          <span className="text-[10px] font-black text-white bg-white/10 px-3 py-1 border border-white/20 uppercase flex items-center gap-2">
                            <span className="w-1.5 h-1.5 bg-[#FF5500]" />
                            {selectedGuildId 
                              ? `Servidor: ${guilds.find(g => g.id === selectedGuildId)?.name || selectedGuildId}`
                              : 'Configuración Global (Valores por defecto del Bot)'}
                          </span>
                        </div>
                      </div>

                      <div className="flex flex-col sm:flex-row items-stretch sm:items-center gap-2 shrink-0">
                        {/* Selector de Modo de Vista */}
                        <div className="flex items-center border border-white/20 bg-black p-0.5">
                          <button
                            onClick={() => setFeaturesViewMode('grouped')}
                            className={`flex items-center gap-1.5 px-3 py-2 text-[10px] font-black uppercase transition-colors cursor-pointer ${
                              featuresViewMode === 'grouped'
                                ? 'bg-white text-black'
                                : 'text-neutral-400 hover:text-white'
                            }`}
                            title="Ver organizado por categorías temáticas"
                          >
                            <Layers className="w-3.5 h-3.5" />
                            <span>Por Categorías</span>
                          </button>
                          <button
                            onClick={() => setFeaturesViewMode('list')}
                            className={`flex items-center gap-1.5 px-3 py-2 text-[10px] font-black uppercase transition-colors cursor-pointer ${
                              featuresViewMode === 'list'
                                ? 'bg-white text-black'
                                : 'text-neutral-400 hover:text-white'
                            }`}
                            title="Ver en lista compacta / tabla"
                          >
                            <ListFilter className="w-3.5 h-3.5" />
                            <span>Lista Tabla</span>
                          </button>
                        </div>

                        {/* Botón de Sincronización */}
                        <button
                          onClick={handleRefreshFeatures}
                          disabled={refreshingFeatures}
                          className="flex items-center justify-center gap-2 px-4 py-2.5 bg-white text-black hover:bg-neutral-200 text-xs font-black uppercase tracking-tighter rounded-none cursor-pointer transition-all active:translate-y-[1px]"
                        >
                          <RefreshCw className={`w-3.5 h-3.5 ${refreshingFeatures ? 'animate-spin' : ''}`} />
                          <span>{refreshingFeatures ? 'Sincronizando...' : 'Verificar DB'}</span>
                        </button>
                      </div>
                    </div>
                  </div>

                  {/* Tablero KPI Estadístico Superior */}
                  <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
                    <div className="bg-[#111] border-2 border-white/10 p-4 space-y-2">
                      <div className="flex items-center justify-between text-neutral-400">
                        <span className="text-[10px] font-black uppercase tracking-wider">TOTAL REGISTRADOS</span>
                        <Sliders className="w-3.5 h-3.5 text-neutral-500" />
                      </div>
                      <div className="text-2xl font-black text-white">{featureStats.total}</div>
                      <div className="space-y-1">
                        <div className="w-full bg-neutral-900 h-1.5 border border-white/10 overflow-hidden">
                          <div 
                            className="bg-emerald-500 h-full transition-all duration-500"
                            style={{ width: `${featureStats.percent}%` }}
                          />
                        </div>
                        <div className="flex justify-between text-[9px] text-neutral-400 font-black uppercase">
                          <span className="text-emerald-400">{featureStats.active} Activos</span>
                          <span className="text-neutral-500">{featureStats.standby} Standby</span>
                        </div>
                      </div>
                    </div>

                    <div className="bg-[#111] border-2 border-white/10 p-4 space-y-2">
                      <div className="flex items-center justify-between text-neutral-400">
                        <span className="text-[10px] font-black uppercase tracking-wider">MÓDULOS DE SISTEMA</span>
                        <Radio className="w-3.5 h-3.5 text-blue-400" />
                      </div>
                      <div className="flex items-baseline gap-2">
                        <span className="text-2xl font-black text-white">{featureStats.modulesActive}</span>
                        <span className="text-xs text-neutral-500">/ {featureStats.modulesTotal} activos</span>
                      </div>
                      <p className="text-[9px] text-neutral-400 uppercase">
                        Automatizaciones, música, voz y monitoreo
                      </p>
                    </div>

                    <div className="bg-[#111] border-2 border-white/10 p-4 space-y-2">
                      <div className="flex items-center justify-between text-neutral-400">
                        <span className="text-[10px] font-black uppercase tracking-wider">COMANDOS SLASH</span>
                        <Power className="w-3.5 h-3.5 text-amber-400" />
                      </div>
                      <div className="flex items-baseline gap-2">
                        <span className="text-2xl font-black text-white">{featureStats.commandsActive}</span>
                        <span className="text-xs text-neutral-500">/ {featureStats.commandsTotal} activos</span>
                      </div>
                      <p className="text-[9px] text-neutral-400 uppercase">
                        Comandos interactivos registrados en Discord
                      </p>
                    </div>

                    <div className="bg-[#111] border-2 border-white/10 p-4 space-y-2">
                      <div className="flex items-center justify-between text-neutral-400">
                        <span className="text-[10px] font-black uppercase tracking-wider">CANALES VINCULADOS</span>
                        <ExternalLink className="w-3.5 h-3.5 text-purple-400" />
                      </div>
                      <div className="flex items-baseline gap-2">
                        <span className="text-2xl font-black text-white">{featureStats.channelsAssigned}</span>
                        <span className="text-xs text-neutral-500">/ {featureStats.channelsTotal} requeridos</span>
                      </div>
                      <p className="text-[9px] text-neutral-400 uppercase">
                        {featureStats.channelsAssigned === featureStats.channelsTotal ? 'Todos los canales asignados' : 'Algunos canales pendientes de asignar'}
                      </p>
                    </div>
                  </div>

                  {/* Barra de Filtro, Búsqueda y Acciones por Lote */}
                  <div className="bg-[#111] border-2 border-white/10 p-4 space-y-3">
                    <div className="flex flex-col lg:flex-row lg:items-center justify-between gap-3">
                      {/* Filtros de Tipo y Estado */}
                      <div className="flex flex-wrap items-center gap-2">
                        {/* Tipo */}
                        <div className="flex items-center border border-white/15 bg-black p-0.5">
                          <button
                            onClick={() => setFeaturesCategory('all')}
                            className={`px-2.5 py-1 text-[10px] font-black uppercase transition-colors cursor-pointer ${
                              featuresCategory === 'all'
                                ? 'bg-white text-black'
                                : 'text-neutral-400 hover:text-white'
                            }`}
                          >
                            Todos ({featureFlags.length})
                          </button>
                          <button
                            onClick={() => setFeaturesCategory('modules')}
                            className={`px-2.5 py-1 text-[10px] font-black uppercase transition-colors cursor-pointer ${
                              featuresCategory === 'modules'
                                ? 'bg-white text-black'
                                : 'text-neutral-400 hover:text-white'
                            }`}
                          >
                            Módulos ({featureStats.modulesTotal})
                          </button>
                          <button
                            onClick={() => setFeaturesCategory('commands')}
                            className={`px-2.5 py-1 text-[10px] font-black uppercase transition-colors cursor-pointer ${
                              featuresCategory === 'commands'
                                ? 'bg-white text-black'
                                : 'text-neutral-400 hover:text-white'
                            }`}
                          >
                            Comandos ({featureStats.commandsTotal})
                          </button>
                        </div>

                        {/* Estado */}
                        <div className="flex items-center border border-white/15 bg-black p-0.5">
                          <button
                            onClick={() => setFeaturesStatusFilter('all')}
                            className={`px-2 py-1 text-[10px] font-black uppercase transition-colors cursor-pointer ${
                              featuresStatusFilter === 'all'
                                ? 'bg-neutral-200 text-black'
                                : 'text-neutral-400 hover:text-white'
                            }`}
                          >
                            Todos
                          </button>
                          <button
                            onClick={() => setFeaturesStatusFilter('active')}
                            className={`px-2 py-1 text-[10px] font-black uppercase transition-colors cursor-pointer ${
                              featuresStatusFilter === 'active'
                                ? 'bg-emerald-500 text-black'
                                : 'text-neutral-400 hover:text-white'
                            }`}
                          >
                            Activos ({featureStats.active})
                          </button>
                          <button
                            onClick={() => setFeaturesStatusFilter('standby')}
                            className={`px-2 py-1 text-[10px] font-black uppercase transition-colors cursor-pointer ${
                              featuresStatusFilter === 'standby'
                                ? 'bg-neutral-600 text-white'
                                : 'text-neutral-400 hover:text-white'
                            }`}
                          >
                            Standby ({featureStats.standby})
                          </button>
                        </div>
                      </div>

                      {/* Buscador y Acciones Masivas */}
                      <div className="flex flex-col sm:flex-row items-stretch sm:items-center gap-2">
                        {/* Buscador */}
                        <div className="relative w-full sm:w-64">
                          <input
                            type="text"
                            value={featuresSearch}
                            onChange={(e) => setFeaturesSearch(e.target.value)}
                            placeholder="Buscar por nombre, key o canal..."
                            className="w-full bg-[#0A0A0A] border border-white/20 text-xs text-white px-3 py-1.5 outline-none focus:border-white font-mono uppercase placeholder:text-neutral-600"
                          />
                          {featuresSearch && (
                            <button
                              onClick={() => setFeaturesSearch('')}
                              className="absolute right-2 top-1.5 text-neutral-500 hover:text-white text-xs cursor-pointer font-black"
                            >
                              ×
                            </button>
                          )}
                        </div>

                        {/* Botones de Acción Masiva */}
                        <div className="flex items-center gap-1.5">
                          <button
                            onClick={() => handleBatchToggleFeatures('enable_all')}
                            disabled={batchUpdatingFeatures}
                            className="px-2.5 py-1.5 bg-emerald-500/20 text-emerald-300 border border-emerald-500/30 hover:bg-emerald-500/30 text-[9px] font-black uppercase transition-colors cursor-pointer disabled:opacity-50"
                            title="Activar todos los servicios mostrados"
                          >
                            Activar Todos
                          </button>
                          <button
                            onClick={() => handleBatchToggleFeatures('disable_all')}
                            disabled={batchUpdatingFeatures}
                            className="px-2.5 py-1.5 bg-neutral-800 text-neutral-400 border border-white/10 hover:bg-neutral-700 hover:text-white text-[9px] font-black uppercase transition-colors cursor-pointer disabled:opacity-50"
                            title="Desactivar todos los servicios mostrados"
                          >
                            Desactivar Todos
                          </button>
                        </div>
                      </div>
                    </div>

                    {/* Filtro por Categorías Temáticas (Pills con Iconos) */}
                    <div className="pt-2 border-t border-white/5 flex flex-wrap items-center gap-1.5">
                      <span className="text-[9px] font-black uppercase tracking-widest text-neutral-500 mr-1">
                        CATEGORÍAS:
                      </span>
                      <button
                        onClick={() => setFeaturesDomainFilter('all')}
                        className={`px-2.5 py-1 text-[9px] font-black uppercase border transition-colors cursor-pointer ${
                          featuresDomainFilter === 'all'
                            ? 'bg-white text-black border-white'
                            : 'bg-black text-neutral-400 border-white/10 hover:border-white/30'
                        }`}
                      >
                        Todas ({featureFlags.length})
                      </button>
                      {(Object.entries(CATEGORY_META) as [FeatureCategory, typeof CATEGORY_META[FeatureCategory]][]).map(([catKey, meta]) => {
                        const count = featureFlags.filter(f => f.category === catKey).length;
                        const activeCount = featureFlags.filter(f => f.category === catKey && f.enabled).length;
                        const isSelected = featuresDomainFilter === catKey;
                        const IconComponent = meta.icon;
                        return (
                          <button
                            key={catKey}
                            onClick={() => setFeaturesDomainFilter(isSelected ? 'all' : catKey)}
                            className={`flex items-center gap-1.5 px-2.5 py-1 text-[9px] font-black uppercase border transition-all cursor-pointer ${
                              isSelected
                                ? 'bg-white text-black border-white shadow-[0_0_10px_rgba(255,255,255,0.2)]'
                                : 'bg-[#0E0E0E] text-neutral-300 border-white/10 hover:border-white/30 hover:bg-white/[0.03]'
                            }`}
                          >
                            <IconComponent className={`w-3 h-3 ${isSelected ? 'text-black' : meta.color}`} />
                            <span>{meta.label}</span>
                            <span className={`text-[8px] font-mono px-1 py-0.2 border ${isSelected ? 'bg-black text-white border-black' : 'bg-white/5 text-neutral-400 border-white/10'}`}>
                              {activeCount}/{count}
                            </span>
                          </button>
                        );
                      })}
                    </div>
                  </div>

                  {/* VISTA A: AGRUPADA POR CATEGORÍAS */}
                  {featuresViewMode === 'grouped' && (
                    <div className="space-y-6">
                      {(Object.entries(CATEGORY_META) as [FeatureCategory, typeof CATEGORY_META[FeatureCategory]][]).map(([catKey, meta]) => {
                        // Flags para esta categoría según los filtros activos
                        const categoryFlags = filteredFeatureFlags.filter(f => f.category === catKey);
                        if (categoryFlags.length === 0) return null;

                        const allFlagsInCat = featureFlags.filter(f => f.category === catKey);
                        const activeInCat = allFlagsInCat.filter(f => f.enabled).length;
                        const IconComponent = meta.icon;

                        return (
                          <div 
                            key={catKey}
                            className="bg-[#111] border-2 border-white/10 rounded-none overflow-hidden shadow-[10px_10px_0px_rgba(0,0,0,0.5)]"
                          >
                            {/* Cabecera de Categoría */}
                            <div className="p-5 bg-gradient-to-r from-[#181818] via-[#141414] to-[#101010] border-b border-white/10 flex flex-col sm:flex-row sm:items-center justify-between gap-4">
                              <div className="flex items-start gap-3.5">
                                <div className={`p-2.5 border ${meta.borderColor} ${meta.bgColor} rounded-none shrink-0`}>
                                  <IconComponent className={`w-5 h-5 ${meta.color}`} />
                                </div>
                                <div>
                                  <div className="flex items-center gap-2">
                                    <h4 className="text-base font-black text-white uppercase tracking-tight">
                                      {meta.label}
                                    </h4>
                                    <span className={`text-[9px] font-black uppercase px-2 py-0.5 border ${meta.badgeBg}`}>
                                      {meta.badge}
                                    </span>
                                  </div>
                                  <p className="text-[11px] text-neutral-400 uppercase tracking-tight mt-0.5">
                                    {meta.desc}
                                  </p>
                                </div>
                              </div>

                              <div className="flex items-center gap-2 self-start sm:self-auto shrink-0">
                                <span className={`text-[10px] font-black font-mono uppercase px-2.5 py-1 border ${
                                  activeInCat === allFlagsInCat.length
                                    ? 'bg-emerald-500/20 text-emerald-300 border-emerald-500/30'
                                    : activeInCat > 0
                                      ? 'bg-amber-500/20 text-amber-300 border-amber-500/30'
                                      : 'bg-neutral-800 text-neutral-400 border-white/10'
                                }`}>
                                  {activeInCat} / {allFlagsInCat.length} ACTIVOS
                                </span>
                                <button
                                  onClick={() => handleBatchToggleFeatures('toggle_category', catKey)}
                                  disabled={batchUpdatingFeatures}
                                  className="px-2.5 py-1 bg-white/5 hover:bg-white/10 text-neutral-300 border border-white/15 text-[9px] font-black uppercase cursor-pointer transition-colors"
                                  title="Alternar estado de todos los servicios en esta categoría"
                                >
                                  Alternar Categoría
                                </button>
                              </div>
                            </div>

                            {/* Grid 2 Columnas de Servicios */}
                            <div className="grid grid-cols-1 md:grid-cols-2 gap-3.5 p-4 md:p-5 bg-[#0A0A0A]">
                              {categoryFlags.map(flag => {
                                const isToggling = togglingFeatureKey === flag.key;
                                const channelVal = flag.channelKey ? (config as any)?.[flag.channelKey] : null;
                                const hasChannelConfigured = channelVal && String(channelVal).trim() !== '' && !String(channelVal).startsWith('ID_');

                                return (
                                  <div 
                                    key={flag.key}
                                    className={`p-4 border transition-all flex flex-col justify-between gap-3.5 ${
                                      flag.enabled 
                                        ? 'bg-[#121212] border-white/15 hover:border-white/30' 
                                        : 'bg-[#0E0E0E] border-white/5 opacity-75 hover:opacity-100 hover:border-white/15'
                                    }`}
                                  >
                                    {/* Fila Superior: Badge Tipo, Nombre y Status Dot */}
                                    <div className="space-y-1.5">
                                      <div className="flex items-center justify-between gap-2">
                                        <div className="flex items-center gap-2 flex-wrap">
                                          <span className={`text-[8px] font-black uppercase px-1.5 py-0.5 border ${
                                            flag.type === 'module'
                                              ? 'bg-blue-500/15 text-blue-400 border-blue-500/30'
                                              : 'bg-amber-500/15 text-amber-400 border-amber-500/30'
                                          }`}>
                                            {flag.type === 'module' ? 'MÓDULO' : 'COMANDO'}
                                          </span>
                                          <h5 className="text-xs font-black uppercase tracking-tight text-white">
                                            {flag.name}
                                          </h5>
                                        </div>

                                        <div className="flex items-center gap-1.5 shrink-0">
                                          <span className={`w-2 h-2 rounded-full ${
                                            flag.enabled ? 'bg-emerald-400 animate-pulse' : 'bg-neutral-600'
                                          }`} />
                                          <span className={`text-[9px] font-black uppercase ${
                                            flag.enabled ? 'text-emerald-400' : 'text-neutral-500'
                                          }`}>
                                            {flag.enabled ? 'ACTIVO' : 'STANDBY'}
                                          </span>
                                        </div>
                                      </div>

                                      <div className="text-[9px] text-neutral-500 font-mono">
                                        key: <span className="text-neutral-400 font-bold">{flag.key}</span>
                                      </div>

                                      <p className="text-[11px] text-neutral-300 leading-relaxed uppercase">
                                        {flag.desc}
                                      </p>
                                    </div>

                                    {/* Estado del Canal Discord (si aplica) */}
                                    {flag.channelKey && (
                                      <div className="pt-2 border-t border-white/5 flex items-center justify-between text-[9px] font-mono">
                                        <span className="text-neutral-500 uppercase font-black">
                                          {flag.channelLabel || 'CANAL'}:
                                        </span>
                                        {hasChannelConfigured ? (
                                          <span className="text-emerald-400 bg-emerald-500/10 border border-emerald-500/20 px-2 py-0.5 flex items-center gap-1 font-bold">
                                            <span>#</span>{channelVal}
                                          </span>
                                        ) : (
                                          <span className="text-amber-400 bg-amber-500/10 border border-amber-500/20 px-2 py-0.5 flex items-center gap-1 font-bold">
                                            ⚠️ No Asignado
                                          </span>
                                        )}
                                      </div>
                                    )}

                                    {/* Botón de Toggle con Feedback */}
                                    <button
                                      onClick={() => handleToggleFeature(flag.key)}
                                      disabled={isToggling}
                                      className={`w-full py-2 px-3 border text-[10px] font-mono font-black uppercase rounded-none transition-all cursor-pointer flex items-center justify-center gap-2 ${
                                        isToggling
                                          ? 'bg-neutral-800 text-neutral-400 border-white/20 cursor-wait'
                                          : flag.enabled 
                                            ? 'bg-emerald-500/20 text-emerald-300 border-emerald-500/40 hover:bg-emerald-500/30' 
                                            : 'bg-neutral-800 text-neutral-400 border-white/10 hover:bg-neutral-700 hover:text-white'
                                      }`}
                                    >
                                      {isToggling ? (
                                        <>
                                          <RefreshCw className="w-3 h-3 animate-spin" />
                                          <span>GUARDANDO CAMBIOS...</span>
                                        </>
                                      ) : flag.enabled ? (
                                        <>
                                          <span className="w-1.5 h-1.5 rounded-full bg-emerald-400" />
                                          <span>HABILITADO • CLICK PARA DESACTIVAR</span>
                                        </>
                                      ) : (
                                        <>
                                          <span className="w-1.5 h-1.5 rounded-full bg-neutral-600" />
                                          <span>DESHABILITADO • CLICK PARA ACTIVAR</span>
                                        </>
                                      )}
                                    </button>
                                  </div>
                                );
                              })}
                            </div>
                          </div>
                        );
                      })}
                    </div>
                  )}

                  {/* VISTA B: LISTA TABLA COMPACTA */}
                  {featuresViewMode === 'list' && (
                    <div className="bg-[#111] border-2 border-white/10 overflow-hidden shadow-[10px_10px_0px_rgba(0,0,0,0.5)] rounded-none">
                      <div className="overflow-x-auto">
                        <table className="w-full text-left font-mono border-collapse">
                          <thead>
                            <tr className="bg-black/60 border-b-2 border-white/10 text-[9px] font-black uppercase text-neutral-400 tracking-wider">
                              <th className="p-3 w-12 text-center">ESTADO</th>
                              <th className="p-3">FUNCIONALIDAD / SERVICIO</th>
                              <th className="p-3">CATEGORÍA</th>
                              <th className="p-3">TIPO</th>
                              <th className="p-3">CANAL ASOCIADO</th>
                              <th className="p-3 text-right">ACCIÓN</th>
                            </tr>
                          </thead>
                          <tbody className="divide-y divide-white/5 text-xs">
                            {filteredFeatureFlags.map(flag => {
                              const isToggling = togglingFeatureKey === flag.key;
                              const channelVal = flag.channelKey ? (config as any)?.[flag.channelKey] : null;
                              const hasChannelConfigured = channelVal && String(channelVal).trim() !== '' && !String(channelVal).startsWith('ID_');
                              const catMeta = flag.category ? CATEGORY_META[flag.category] : null;

                              return (
                                <tr key={flag.key} className="hover:bg-white/[0.02] transition-colors">
                                  {/* Estado Dot */}
                                  <td className="p-3 text-center">
                                    <span className={`inline-block w-2.5 h-2.5 rounded-full ${
                                      flag.enabled ? 'bg-emerald-400 animate-pulse' : 'bg-neutral-600'
                                    }`} />
                                  </td>

                                  {/* Nombre y Clave */}
                                  <td className="p-3">
                                    <div className="font-black text-white uppercase">{flag.name}</div>
                                    <div className="text-[10px] text-neutral-500 font-mono">({flag.key})</div>
                                    <div className="text-[10px] text-neutral-400 uppercase mt-0.5 max-w-md line-clamp-1">
                                      {flag.desc}
                                    </div>
                                  </td>

                                  {/* Categoría */}
                                  <td className="p-3">
                                    {catMeta ? (
                                      <span className={`text-[8px] font-black uppercase px-2 py-0.5 border ${catMeta.badgeBg}`}>
                                        {catMeta.label}
                                      </span>
                                    ) : (
                                      <span className="text-[8px] text-neutral-500 uppercase font-mono">GENERAL</span>
                                    )}
                                  </td>

                                  {/* Tipo */}
                                  <td className="p-3">
                                    <span className={`text-[8px] font-black uppercase px-1.5 py-0.5 border ${
                                      flag.type === 'module'
                                        ? 'bg-blue-500/15 text-blue-400 border-blue-500/30'
                                        : 'bg-amber-500/15 text-amber-400 border-amber-500/30'
                                    }`}>
                                      {flag.type === 'module' ? 'MÓDULO' : 'COMANDO'}
                                    </span>
                                  </td>

                                  {/* Canal */}
                                  <td className="p-3">
                                    {flag.channelKey ? (
                                      hasChannelConfigured ? (
                                        <span className="text-emerald-400 bg-emerald-500/10 border border-emerald-500/20 px-2 py-0.5 text-[9px] font-mono">
                                          #{channelVal}
                                        </span>
                                      ) : (
                                        <span className="text-amber-400 bg-amber-500/10 border border-amber-500/20 px-2 py-0.5 text-[9px] font-mono">
                                          ⚠️ No Asignado
                                        </span>
                                      )
                                    ) : (
                                      <span className="text-neutral-600 text-[10px]">—</span>
                                    )}
                                  </td>

                                  {/* Botón Acción */}
                                  <td className="p-3 text-right">
                                    <button
                                      onClick={() => handleToggleFeature(flag.key)}
                                      disabled={isToggling}
                                      className={`px-3 py-1.5 border text-[9px] font-black uppercase transition-colors cursor-pointer ${
                                        isToggling
                                          ? 'bg-neutral-800 text-neutral-400 border-white/20'
                                          : flag.enabled
                                            ? 'bg-emerald-500/20 text-emerald-300 border-emerald-500/40 hover:bg-emerald-500/30'
                                            : 'bg-neutral-800 text-neutral-400 border-white/10 hover:bg-neutral-700 hover:text-white'
                                      }`}
                                    >
                                      {isToggling ? 'GUARDANDO...' : flag.enabled ? 'ACTIVO' : 'STANDBY'}
                                    </button>
                                  </td>
                                </tr>
                              );
                            })}
                          </tbody>
                        </table>
                      </div>
                    </div>
                  )}

                  {/* Estado vacío cuando la búsqueda o filtros no devuelven nada */}
                  {filteredFeatureFlags.length === 0 && (
                    <div className="bg-[#111] border-2 border-white/10 p-12 text-center rounded-none shadow-[10px_10px_0px_rgba(0,0,0,0.5)] space-y-4">
                      <div className="text-neutral-500 font-mono text-3xl">🔍</div>
                      <div className="space-y-1">
                        <h4 className="text-base font-black text-white uppercase">
                          No se encontraron funcionalidades coincidentes
                        </h4>
                        <p className="text-xs text-neutral-400 uppercase font-mono max-w-md mx-auto">
                          Ningún servicio coincide con los filtros aplicados ({featuresSearch ? `búsqueda: "${featuresSearch}"` : 'filtros de categoría o estado'}).
                        </p>
                      </div>
                      <button
                        onClick={() => {
                          setFeaturesSearch('');
                          setFeaturesCategory('all');
                          setFeaturesStatusFilter('all');
                          setFeaturesDomainFilter('all');
                        }}
                        className="px-4 py-2 bg-white text-black hover:bg-neutral-200 text-xs font-black uppercase cursor-pointer"
                      >
                        Restablecer Todos los Filtros
                      </button>
                    </div>
                  )}

                </div>
              )}

              {/* VIEW: MENSAJES MASIVOS */}
              {activeTab === 'messages' && (
                <div className="space-y-6 animate-fade-in max-w-4xl mx-auto">

                  <div className="bg-[#111] border-2 border-white/10 p-8 rounded-none">
                    <h3 className="text-xl font-black text-white uppercase tracking-tighter italic mb-1">DIFUSIÓN MASIVA (DM)</h3>
                    <p className="text-xs text-neutral-400 uppercase tracking-tight font-mono">Selecciona un usuario para enviarle un mensaje individual, o deja en blanco para enviar a todos los verificados.</p>
                  </div>

                  <div className="bg-[#111] border-2 border-white/10 p-8 space-y-6 shadow-[10px_10px_0px_rgba(0,0,0,0.5)] rounded-none">

                    {/* Selector de usuario */}
                    <div className="flex flex-col gap-2">
                      <label className="text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">
                        Destinatario <span className="text-neutral-600 normal-case">(opcional — dejar vacío para envío masivo)</span>
                      </label>

                      {broadcastTarget ? (
                        <div className="flex items-center justify-between bg-[#0A0A0A] border-2 border-[#FF5500]/50 px-4 py-3">
                          <div>
                            <span className="text-xs font-black text-white uppercase font-mono">{broadcastTarget.faceitNickname}</span>
                            <span className="text-[10px] text-neutral-500 font-mono ml-3">{broadcastTarget.discordId}</span>
                          </div>
                          <button onClick={() => { setBroadcastTarget(null); setBroadcastTargetSearch(''); }} className="text-neutral-400 hover:text-red-400 transition-colors text-xs font-black cursor-pointer">
                            QUITAR ×
                          </button>
                        </div>
                      ) : (
                        <div className="relative">
                          <input
                            type="text"
                            value={broadcastTargetSearch}
                            onChange={(e) => setBroadcastTargetSearch(e.target.value)}
                            placeholder="Buscar usuario por nickname de Faceit..."
                            className="w-full bg-[#0A0A0A] border-2 border-white/10 text-xs font-mono text-white rounded-none px-4 py-3.5 outline-none focus:border-white/30 transition-colors"
                          />
                          {broadcastTargetSearch.trim() && (
                            <div className="absolute top-full left-0 right-0 z-10 bg-[#1A1A1A] border-2 border-white/10 border-t-0 max-h-48 overflow-y-auto">
                              {users
                                .filter(u => u.faceitNickname.toLowerCase().includes(broadcastTargetSearch.toLowerCase()) && u.status === 'Verificado')
                                .slice(0, 8)
                                .map(u => (
                                  <button
                                    key={u.discordId}
                                    onClick={() => { setBroadcastTarget({ discordId: u.discordId, faceitNickname: u.faceitNickname }); setBroadcastTargetSearch(''); }}
                                    className="w-full text-left px-4 py-3 hover:bg-white/5 transition-colors flex items-center justify-between cursor-pointer"
                                  >
                                    <span className="text-xs font-black text-white font-mono uppercase">{u.faceitNickname}</span>
                                    <span className="text-[10px] text-neutral-500 font-mono">{u.discordId}</span>
                                  </button>
                                ))}
                              {users.filter(u => u.faceitNickname.toLowerCase().includes(broadcastTargetSearch.toLowerCase()) && u.status === 'Verificado').length === 0 && (
                                <div className="px-4 py-3 text-xs text-neutral-500 font-mono">No se encontraron usuarios</div>
                              )}
                            </div>
                          )}
                        </div>
                      )}
                    </div>

                    {/* Mensaje */}
                    <div className="flex flex-col gap-2">
                      <label className="text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Contenido del Mensaje</label>
                      <textarea
                        value={broadcastMessage}
                        onChange={(e) => setBroadcastMessage(e.target.value)}
                        placeholder="Escribe el mensaje aquí..."
                        className="bg-[#0A0A0A] border-2 border-white/10 text-sm font-mono text-white rounded-none px-4 py-3.5 outline-none focus:border-white transition-colors h-40 resize-none"
                      />
                    </div>

                    {/* URL de la Imagen */}
                    <div className="flex flex-col gap-2">
                      <label className="text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">
                        URL de la Imagen <span className="text-neutral-600 normal-case">(opcional — ej: enlace directo a un archivo png/jpg/gif)</span>
                      </label>
                      <input
                        type="url"
                        value={broadcastImageUrl}
                        onChange={(e) => setBroadcastImageUrl(e.target.value)}
                        placeholder="https://ejemplo.com/imagen.png"
                        className="bg-[#0A0A0A] border-2 border-white/10 text-xs font-mono text-white rounded-none px-4 py-3.5 outline-none focus:border-white transition-colors"
                      />
                    </div>

                    {/* Estado y botón */}
                    <div className="flex items-center justify-between gap-4">
                      <div className="flex-1">
                        {broadcastStatus.status === 'sending' && (
                          <div className="flex items-center gap-3">
                            <div className="w-4 h-4 border-2 border-white border-t-transparent rounded-full animate-spin"></div>
                            <span className="text-xs font-mono font-bold text-white uppercase tracking-wider">{broadcastStatus.msg}</span>
                          </div>
                        )}
                        {broadcastStatus.status === 'success' && (
                          <span className="text-xs font-mono font-bold text-green-400 uppercase tracking-wider">{broadcastStatus.msg}</span>
                        )}
                        {broadcastStatus.status === 'error' && (
                          <span className="text-xs font-mono font-bold text-red-400 uppercase tracking-wider">{broadcastStatus.msg}</span>
                        )}
                      </div>

                      <button
                        onClick={handleBroadcast}
                        disabled={broadcastStatus.status === 'sending' || !broadcastMessage.trim()}
                        className={`px-8 py-3 font-black uppercase tracking-widest text-xs transition-colors rounded-none disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer ${
                          broadcastTarget
                            ? 'bg-[#5865F2] hover:bg-white text-white hover:text-black'
                            : 'bg-[#FF5500] hover:bg-white text-white hover:text-black'
                        }`}
                      >
                        {broadcastTarget ? `ENVIAR A ${broadcastTarget.faceitNickname.toUpperCase()}` : 'ENVIAR A TODOS'}
                      </button>
                    </div>
                  </div>
                </div>
              )}

              {/* VIEW: SIMULADOR DE WEBHOOKS */}
              {activeTab === 'simulator' && (
                <div className="space-y-6 animate-fade-in max-w-4xl mx-auto">
                  <div className="bg-[#111] border-2 border-white/10 p-8 rounded-none">
                    <h3 className="text-xl font-black text-white uppercase tracking-tighter italic mb-1">SIMULADOR DE WEBHOOKS DE FACEIT</h3>
                    <p className="text-xs text-neutral-400 uppercase tracking-tight font-mono">Dispara eventos simulados para probar las notificaciones del bot y los canales de voz.</p>
                  </div>

                  <div className="bg-[#111] border-2 border-white/10 p-8 space-y-6 shadow-[10px_10px_0px_rgba(0,0,0,0.5)] rounded-none font-mono">
                    
                    {/* Select Event Type */}
                    <div className="flex flex-col gap-2">
                      <label className="text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Tipo de Evento Webhook</label>
                      <select
                        value={simulatedEvent}
                        onChange={(e) => setSimulatedEvent(e.target.value)}
                        className="bg-[#0A0A0A] text-xs font-black text-white border-2 border-white/20 rounded-none px-4 py-3 outline-none focus:border-white transition-colors cursor-pointer uppercase tracking-wider"
                      >
                        <option value="match_status_configuring">PARTIDA EN CONFIGURACIÓN (match_status_configuring)</option>
                        <option value="match_status_starting">PARTIDA INICIANDO (match_status_starting)</option>
                        <option value="match_status_finished">PARTIDA FINALIZADA (match_status_finished)</option>
                        <option value="match_status_cancelled">PARTIDA CANCELADA (match_status_cancelled)</option>
                      </select>
                    </div>

                    <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
                      <div className="flex flex-col gap-1.5">
                        <label className="text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">ID de Partida (Match ID)</label>
                        <input
                          type="text"
                          value={simMatchId}
                          onChange={(e) => setSimMatchId(e.target.value)}
                          className="bg-[#0A0A0A] border-2 border-white/10 text-xs font-mono text-white rounded-none px-4 py-3 outline-none focus:border-white transition-colors"
                        />
                      </div>
                      <div className="flex flex-col gap-1.5">
                        <label className="text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Nombre de Mapa (Map)</label>
                        <input
                          type="text"
                          value={simMap}
                          onChange={(e) => setSimMap(e.target.value)}
                          className="bg-[#0A0A0A] border-2 border-white/10 text-xs font-mono text-white rounded-none px-4 py-3 outline-none focus:border-white transition-colors"
                        />
                      </div>
                      <div className="flex flex-col gap-1.5">
                        <label className="text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Nombre Equipo A (Faction 1)</label>
                        <input
                          type="text"
                          value={simTeamA}
                          onChange={(e) => setSimTeamA(e.target.value)}
                          className="bg-[#0A0A0A] border-2 border-white/10 text-xs font-mono text-white rounded-none px-4 py-3 outline-none focus:border-white transition-colors"
                        />
                      </div>
                      <div className="flex flex-col gap-1.5">
                        <label className="text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Nombre Equipo B (Faction 2)</label>
                        <input
                          type="text"
                          value={simTeamB}
                          onChange={(e) => setSimTeamB(e.target.value)}
                          className="bg-[#0A0A0A] border-2 border-white/10 text-xs font-mono text-white rounded-none px-4 py-3 outline-none focus:border-white transition-colors"
                        />
                      </div>
                    </div>

                    {simulatedEvent === 'match_status_finished' && (
                      <div className="grid grid-cols-1 md:grid-cols-3 gap-6">
                        <div className="flex flex-col gap-1.5">
                          <label className="text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Ganador (Winner Faction)</label>
                          <select
                            value={simWinner}
                            onChange={(e) => setSimWinner(e.target.value)}
                            className="bg-[#0A0A0A] text-xs font-black text-white border-2 border-white/20 rounded-none px-4 py-3 outline-none focus:border-white transition-colors cursor-pointer"
                          >
                            <option value="faction1">EQUIPO A (faction1)</option>
                            <option value="faction2">EQUIPO B (faction2)</option>
                          </select>
                        </div>
                        <div className="flex flex-col gap-1.5">
                          <label className="text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Rondas Equipo A</label>
                          <input
                            type="number"
                            value={simScoreA}
                            onChange={(e) => setSimScoreA(parseInt(e.target.value, 10) || 0)}
                            className="bg-[#0A0A0A] border-2 border-white/10 text-xs font-mono text-white rounded-none px-4 py-3 outline-none focus:border-white transition-colors"
                          />
                        </div>
                        <div className="flex flex-col gap-1.5">
                          <label className="text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Rondas Equipo B</label>
                          <input
                            type="number"
                            value={simScoreB}
                            onChange={(e) => setSimScoreB(parseInt(e.target.value, 10) || 0)}
                            className="bg-[#0A0A0A] border-2 border-white/10 text-xs font-mono text-white rounded-none px-4 py-3 outline-none focus:border-white transition-colors"
                          />
                        </div>
                      </div>
                    )}

                    {/* Players assignment helper info */}
                    <div className="bg-[#151515] border border-white/5 p-4 text-[11px] text-neutral-400 space-y-2">
                      <p className="font-bold text-white uppercase">ℹ️ INFORMACIÓN DE JUGADORES SIMULADOS:</p>
                      <p>
                        Para hacer las pruebas de traslado de voz y canales, el simulador utilizará automáticamente a los usuarios registrados en tu base de datos y los distribuirá en los rosters de los dos equipos.
                      </p>
                    </div>

                    <div className="flex justify-end pt-4">
                      <button
                        onClick={handleTriggerSimulatedWebhook}
                        disabled={simulatingWebhook}
                        className="px-8 py-3 bg-[#FF5500] hover:bg-white text-white hover:text-black font-black uppercase tracking-widest text-xs transition-colors rounded-none disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
                      >
                        {simulatingWebhook ? 'PROCESANDO EVENTO...' : '🚀 DISPARAR EVENTO SIMULADO'}
                      </button>
                    </div>
                  </div>
                </div>
              )}

              {/* VIEW 8: CONSOLE LOGS */}
              {activeTab === 'logs' && (
                <div className="space-y-6 animate-fade-in">
                  
                  {/* Logs filter bar */}
                  <div className="bg-[#111] border-2 border-white/10 p-8 flex flex-col md:flex-row md:items-center justify-between gap-4 rounded-none">
                    <div>
                      <h3 className="text-xl font-black text-white uppercase tracking-tighter italic mb-1">CONSOLA DE REGISTROS RECIENTES</h3>
                      <p className="text-xs text-neutral-400 uppercase tracking-tight font-mono">Historial detallado de eventos de Discord y Faceit API.</p>
                    </div>

                    <div className="flex flex-wrap items-center gap-4">
                      <button
                        onClick={async () => {
                          showToast('Refrescando logs...', 'info');
                          try {
                            const guildQuery = selectedGuildId ? `?guildId=${selectedGuildId}` : '';
                            const res = await apiFetch(`/api/admin/logs${guildQuery}`);
                            if (res.ok) {
                              setLogs((await res.json()).logs || []);
                            }
                          } catch (e) {
                            showToast('Error al refrescar logs.', 'error');
                          }
                        }}
                        className="flex items-center gap-2 px-4 py-2 bg-white text-black hover:bg-neutral-200 text-[10px] font-mono font-black uppercase rounded-none cursor-pointer border border-white"
                      >
                        <RefreshCw className="w-3.5 h-3.5" />
                        <span>Actualizar Logs</span>
                      </button>

                      {/* Filter buttons */}
                      <div className="flex flex-wrap gap-1.5 p-1 bg-[#0A0A0A] border-2 border-white/10 rounded-none">
                        {['all', 'info', 'warn', 'error', 'success'].map(level => (
                          <button
                            key={level}
                            onClick={() => setLogsFilter(level)}
                            className={`px-3 py-1.5 rounded-none text-[9px] font-mono font-black uppercase cursor-pointer transition-colors border ${
                              logsFilter === level
                                ? 'bg-white text-black border-white'
                                : 'text-neutral-400 border-transparent hover:text-white hover:bg-white/5'
                            }`}
                          >
                            {level === 'all' ? 'TODOS' : level}
                          </button>
                        ))}
                      </div>
                    </div>
                  </div>

                  {/* Log console window layout */}
                  <div className="bg-[#111] border-2 border-white/10 overflow-hidden shadow-[10px_10px_0px_rgba(0,0,0,0.5)] rounded-none">
                    <div className="max-h-[500px] overflow-y-auto">
                      <table className="w-full text-left border-collapse font-mono text-xs">
                        <thead>
                          <tr className="bg-neutral-900 border-b border-white/10 text-neutral-400">
                            <th className="py-4 px-6 text-[10px] font-black uppercase tracking-widest w-28">NIVEL</th>
                            <th className="py-4 px-6 text-[10px] font-black uppercase tracking-widest">MENSAJE DE REGISTRO</th>
                            <th className="py-4 px-6 text-[10px] font-black uppercase tracking-widest w-48 text-right">MARCA DE TIEMPO</th>
                          </tr>
                        </thead>
                        <tbody className="divide-y divide-white/5">
                          {filteredLogs.length === 0 ? (
                            <tr>
                              <td colSpan={3} className="py-8 text-center text-neutral-500 font-mono font-bold uppercase">
                                No hay registros que coincidan con la categoría elegida.
                              </td>
                            </tr>
                          ) : (
                            filteredLogs.map(log => (
                              <tr key={log.id} className="hover:bg-white/[0.02] border-b border-white/5 transition-colors">
                                <td className="py-3 px-6">
                                  <span className={`inline-flex items-center gap-1.5 px-2 py-0.5 rounded-none text-[9px] font-mono font-black uppercase border ${
                                    log.level === 'success' ? 'bg-white text-black border-white' :
                                    log.level === 'warn' ? 'bg-yellow-950/20 text-yellow-500 border-yellow-500/20' :
                                    log.level === 'error' ? 'bg-red-950/20 text-red-500 border-red-500/20' :
                                    'bg-[#1C1C1C] text-neutral-400 border-white/10'
                                  }`}>
                                    {log.level}
                                  </span>
                                </td>
                                <td className="py-3 px-6 text-white font-mono break-all uppercase text-[11px] tracking-tight">{log.message}</td>
                                <td className="py-3 px-6 text-neutral-400 text-right font-mono text-[10px] whitespace-nowrap">{log.timestamp}</td>
                              </tr>
                            ))
                          )}
                        </tbody>
                      </table>
                    </div>
                  </div>

                </div>
              )}

              {/* VIEW 9: SUPPORT CALLS */}
              {activeTab === 'support' && (
                <div className="space-y-6 animate-fade-in">
                  <div className="bg-[#111] border-2 border-white/10 p-8 flex flex-col md:flex-row md:items-center justify-between gap-4 rounded-none">
                    <div>
                      <h3 className="text-xl font-black text-white uppercase tracking-tighter italic mb-1">SOLICITUDES DE SOPORTE</h3>
                      <p className="text-xs text-neutral-400 uppercase tracking-tight font-mono">Listado de alertas de admin enviadas por jugadores desde Discord.</p>
                    </div>
                  </div>

                  <div className="bg-[#111] border-2 border-white/10 overflow-hidden shadow-[10px_10px_0px_rgba(0,0,0,0.5)] rounded-none">
                    <table className="w-full text-left border-collapse font-mono text-xs">
                      <thead>
                        <tr className="bg-neutral-900 border-b border-white/10 text-neutral-400">
                          <th className="py-4 px-6 text-[10px] font-black uppercase tracking-widest w-40">CREADA EN</th>
                          <th className="py-4 px-6 text-[10px] font-black uppercase tracking-widest w-40">JUGADOR (NICKNAME)</th>
                          <th className="py-4 px-6 text-[10px] font-black uppercase tracking-widest">DESCRIPCIÓN DEL PROBLEMA</th>
                          <th className="py-4 px-6 text-[10px] font-black uppercase tracking-widest w-44">ENLACE PARTIDA</th>
                          <th className="py-4 px-6 text-[10px] font-black uppercase tracking-widest w-32">ESTADO</th>
                          <th className="py-4 px-6 text-[10px] font-black uppercase tracking-widest w-40 text-right">ACCIÓN</th>
                        </tr>
                      </thead>
                      <tbody className="divide-y divide-white/5 text-white/90">
                        {supportCalls.length === 0 ? (
                          <tr>
                            <td colSpan={6} className="py-8 text-center text-neutral-500 uppercase tracking-widest text-[10px] font-bold">
                              No hay solicitudes de soporte registradas.
                            </td>
                          </tr>
                        ) : (
                          supportCalls.map((call: any) => {
                            let createdStr = 'Desconocido';
                            if (call.createdAt && call.createdAt._seconds) {
                              createdStr = new Date(call.createdAt._seconds * 1000).toLocaleString('es-ES');
                            } else if (typeof call.createdAt === 'string') {
                              createdStr = call.createdAt;
                            } else if (call.createdAt) {
                              createdStr = new Date(call.createdAt).toLocaleString('es-ES');
                            }
                            return (
                              <tr key={call.id} className="hover:bg-white/5 transition-colors">
                                <td className="py-4 px-6 font-mono text-neutral-400">{createdStr}</td>
                                 <td className="py-4 px-6 font-bold">
                                  <div className="flex flex-col">
                                    <button
                                      onClick={() => {
                                        setStatsSearch(call.faceit_nickname);
                                        setActiveTab('stats');
                                        setTimeout(() => handlePlayerSearch(undefined, call.faceit_nickname), 100);
                                      }}
                                      className="text-[#FF5500] hover:underline font-bold text-left focus:outline-none cursor-pointer"
                                    >
                                      {call.faceit_nickname}
                                    </button>
                                    <span className="text-[9px] text-neutral-500 font-mono">Discord: {call.discord_id}</span>
                                  </div>
                                </td>
                                <td className="py-4 px-6 whitespace-normal max-w-xs">{call.description}</td>
                                <td className="py-4 px-6">
                                  {call.match_url ? (
                                    <a href={call.match_url} target="_blank" rel="noreferrer" className="text-[#FF5500] hover:underline font-bold flex items-center gap-1">
                                      Ver Partida <ArrowUpRight className="w-3.5 h-3.5" />
                                    </a>
                                  ) : (
                                    <span className="text-neutral-500">N/A</span>
                                  )}
                                </td>
                                <td className="py-4 px-6">
                                  {call.status === 'pendiente' ? (
                                    <span className="px-2 py-0.5 text-[9px] bg-red-500/10 border border-red-500/30 text-red-400 font-black uppercase tracking-wider rounded-none">
                                      Pendiente
                                    </span>
                                  ) : (
                                    <div className="flex flex-col gap-0.5">
                                      <span className="px-2 py-0.5 text-[9px] bg-green-500/10 border border-green-500/30 text-green-400 font-black uppercase tracking-wider rounded-none inline-block self-start">
                                        Atendido
                                      </span>
                                      <span className="text-[8px] text-neutral-500 truncate max-w-[120px]">Por: {call.claimedBy}</span>
                                      {(() => {
                                        if (call.claimedAt && call.createdAt) {
                                          const createdMs = call.createdAt._seconds ? call.createdAt._seconds * 1000 : new Date(call.createdAt).getTime();
                                          const claimedMs = typeof call.claimedAt === 'number' ? call.claimedAt : (call.claimedAt._seconds ? call.claimedAt._seconds * 1000 : new Date(call.claimedAt).getTime());
                                          const diffSec = Math.round((claimedMs - createdMs) / 1000);
                                          if (diffSec < 60) {
                                            return <span className="text-[8px] text-neutral-500">⏱️ {diffSec}s</span>;
                                          } else {
                                            return <span className="text-[8px] text-neutral-500">⏱️ {Math.round(diffSec / 60)}m</span>;
                                          }
                                        }
                                        return null;
                                      })()}
                                    </div>
                                  )}
                                </td>
                                <td className="py-4 px-6 text-right">
                                  {call.status === 'pendiente' && (
                                    <button
                                      onClick={async () => {
                                        try {
                                          const res = await apiFetch(`/api/admin/support-calls/${call.id}/resolve`, { method: 'POST' });
                                          if (res.ok) {
                                            showToast('Solicitud marcada como atendida');
                                            loadData();
                                          } else {
                                            showToast('Error al resolver la solicitud', 'error');
                                          }
                                        } catch (e) {
                                          showToast('Error de red al resolver la solicitud', 'error');
                                        }
                                      }}
                                      className="px-3 py-1 bg-white hover:bg-neutral-200 text-black text-[9px] font-black uppercase cursor-pointer rounded-none tracking-tighter"
                                    >
                                      Atender
                                    </button>
                                  )}
                                </td>
                              </tr>
                            );
                          })
                        )}
                      </tbody>
                    </table>
                  </div>
                </div>
              )}

              {/* VIEW 10: MESSAGE TEMPLATES */}
              {activeTab === 'templates' && (
                <div className="space-y-6 animate-fade-in">
                  <div className="bg-[#111] border-2 border-white/10 p-8 flex flex-col md:flex-row md:items-center justify-between gap-4 rounded-none">
                    <div>
                      <h3 className="text-xl font-black text-white uppercase tracking-tighter italic mb-1">PLANTILLAS DE MENSAJES</h3>
                      <p className="text-xs text-neutral-400 uppercase tracking-tight font-mono">Personaliza los embeds y descripciones que el bot envía.</p>
                    </div>
                  </div>

                  <div className="grid grid-cols-1 lg:grid-cols-2 gap-8">
                    {/* Welcome Template Form */}
                    <div className="bg-[#111] border-2 border-white/10 p-8 space-y-4 rounded-none">
                      <h4 className="text-md font-black text-white uppercase tracking-wider border-b border-white/10 pb-2">🎉 MENSAJE DE BIENVENIDA (DM)</h4>
                      
                      <div className="flex flex-col gap-1.5">
                        <label className="text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Título del Embed</label>
                        <input
                          type="text"
                          value={messageTemplates.welcomeTitle || ''}
                          onChange={(e) => setMessageTemplates(prev => ({ ...prev, welcomeTitle: e.target.value }))}
                          className="bg-[#0A0A0A] border border-white/10 text-xs font-mono font-black text-white rounded-none px-4 py-3 outline-none focus:border-white transition-colors"
                        />
                      </div>

                      <div className="flex flex-col gap-1.5">
                        <label className="text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Descripción / Cuerpo del Mensaje</label>
                        <textarea
                          rows={12}
                          value={messageTemplates.welcomeDescription || ''}
                          onChange={(e) => setMessageTemplates(prev => ({ ...prev, welcomeDescription: e.target.value }))}
                          className="bg-[#0A0A0A] border border-white/10 text-xs font-mono text-white rounded-none px-4 py-3 outline-none focus:border-white transition-colors"
                          placeholder="Usa {username}, {clubUrl} y {linkChannelMention} como marcadores de posición dinámicos."
                        />
                        <span className="text-[9px] text-neutral-500 font-mono uppercase">Variables disponibles: &#123;username&#125;, &#123;clubUrl&#125;, &#123;linkChannelMention&#125;</span>
                      </div>

                      <div className="flex flex-col gap-1.5">
                        <label className="text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Color del Embed (HEX)</label>
                        <input
                          type="text"
                          value={messageTemplates.welcomeColor || ''}
                          onChange={(e) => setMessageTemplates(prev => ({ ...prev, welcomeColor: e.target.value }))}
                          className="bg-[#0A0A0A] border border-white/10 text-xs font-mono font-black text-white rounded-none px-4 py-3 outline-none focus:border-white transition-colors w-32"
                          placeholder="#FF5500"
                        />
                      </div>
                    </div>

                    {/* Support Panel Form */}
                    <div className="bg-[#111] border-2 border-white/10 p-8 space-y-4 rounded-none">
                      <h4 className="text-md font-black text-white uppercase tracking-wider border-b border-white/10 pb-2">🚨 PANEL DE LLAMADO DE ADMIN</h4>
                      
                      <div className="flex flex-col gap-1.5">
                        <label className="text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Título del Embed</label>
                        <input
                          type="text"
                          value={messageTemplates.supportTitle || ''}
                          onChange={(e) => setMessageTemplates(prev => ({ ...prev, supportTitle: e.target.value }))}
                          className="bg-[#0A0A0A] border border-white/10 text-xs font-mono font-black text-white rounded-none px-4 py-3 outline-none focus:border-white transition-colors"
                        />
                      </div>

                      <div className="flex flex-col gap-1.5">
                        <label className="text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Descripción / Cuerpo del Mensaje</label>
                        <textarea
                          rows={12}
                          value={messageTemplates.supportDescription || ''}
                          onChange={(e) => setMessageTemplates(prev => ({ ...prev, supportDescription: e.target.value }))}
                          className="bg-[#0A0A0A] border border-white/10 text-xs font-mono text-white rounded-none px-4 py-3 outline-none focus:border-white transition-colors"
                        />
                      </div>

                      <div className="flex flex-col gap-1.5">
                        <label className="text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Color del Embed (HEX)</label>
                        <input
                          type="text"
                          value={messageTemplates.supportColor || ''}
                          onChange={(e) => setMessageTemplates(prev => ({ ...prev, supportColor: e.target.value }))}
                          className="bg-[#0A0A0A] border border-white/10 text-xs font-mono font-black text-white rounded-none px-4 py-3 outline-none focus:border-white transition-colors w-32"
                          placeholder="#FF0000"
                        />
                      </div>
                    </div>
                  </div>

                  <div className="flex justify-end pt-4">
                    <button
                      onClick={async () => {
                        try {
                          const res = await apiFetch('/api/admin/messages', {
                            method: 'POST',
                            body: JSON.stringify(messageTemplates)
                          });
                          if (res.ok) {
                            showToast('Mensajes actualizados correctamente');
                            loadData();
                          } else {
                            showToast('Error al guardar las plantillas', 'error');
                          }
                        } catch (e) {
                          showToast('Error de red al guardar las plantillas', 'error');
                        }
                      }}
                      className="px-8 py-3 bg-white hover:bg-neutral-200 text-black font-black uppercase tracking-widest text-xs transition-colors rounded-none cursor-pointer"
                    >
                      Guardar Cambios
                    </button>
                  </div>
                </div>
              )}

            </main>
          </div>

          {/* Modal 1: VERIFICACIÓN MANUAL */}
          {verifyModalOpen && (
            <div className="fixed inset-0 z-[1000] flex items-center justify-center p-4">
              <div onClick={() => setVerifyModalOpen(false)} className="absolute inset-0 bg-black/90 backdrop-blur-sm" />
              
              <div className="relative bg-[#111] border-2 border-white p-8 shadow-[10px_10px_0px_rgba(0,0,0,0.5)] rounded-none w-full max-w-md z-10 font-mono">
                <h3 className="text-lg font-black text-white uppercase tracking-tighter italic mb-6 flex items-center gap-2">
                  <UserPlus className="w-5 h-5 text-white" />
                  <span>AÑADIR VERIFICACIÓN MANUAL</span>
                </h3>

                <form onSubmit={handleManualVerify} className="space-y-4">
                  <div className="flex flex-col gap-1.5">
                    <label className="text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Discord ID del Usuario</label>
                    <input
                      type="text"
                      placeholder="EJ: 1938472918347"
                      value={manualDiscordId}
                      onChange={(e) => setManualDiscordId(e.target.value)}
                      className="bg-[#0A0A0A] border-2 border-white/10 text-xs font-mono font-black text-white rounded-none px-4 py-3.5 outline-none focus:border-white transition-colors"
                      required
                    />
                  </div>

                  <div className="flex flex-col gap-1.5">
                    <label className="text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Nickname Exacto de Faceit</label>
                    <input
                      type="text"
                      placeholder="EJ: TOXICPLAYER_99"
                      value={manualNickname}
                      onChange={(e) => setManualNickname(e.target.value)}
                      className="bg-[#0A0A0A] border-2 border-white/10 text-xs font-mono font-black text-white rounded-none px-4 py-3.5 outline-none focus:border-white transition-colors"
                      required
                    />
                  </div>

                  <div className="pt-4 flex gap-3">
                    <button
                      type="button"
                      onClick={() => setVerifyModalOpen(false)}
                      className="flex-1 py-4 px-4 text-xs font-black bg-[#1A1A1A] hover:bg-neutral-800 text-white transition-colors cursor-pointer rounded-none border border-white/10 uppercase tracking-tighter"
                    >
                      Cancelar
                    </button>
                    <button
                      type="submit"
                      className="flex-1 py-4 px-4 text-xs font-black bg-white hover:bg-neutral-200 text-black transition-colors cursor-pointer rounded-none border border-white uppercase tracking-tighter"
                    >
                      Vincular Cuenta
                    </button>
                  </div>
                </form>
              </div>
            </div>
          )}

          {/* Modal 2: ASIGNAR ROL MANUAL */}
          {roleModalOpen && (
            <div className="fixed inset-0 z-[1000] flex items-center justify-center p-4">
              <div onClick={() => setRoleModalOpen(false)} className="absolute inset-0 bg-black/90 backdrop-blur-sm" />
              
              <div className="relative bg-[#111] border-2 border-white p-8 shadow-[10px_10px_0px_rgba(0,0,0,0.5)] rounded-none w-full max-w-md z-10 font-mono">
                <h3 className="text-lg font-black text-white mb-6 uppercase tracking-tighter italic">🏷️ ASIGNAR ROL PERSONALIZADO</h3>

                <form onSubmit={handleAssignRoleSubmit} className="space-y-4">
                  
                  <div className="flex flex-col gap-1.5">
                    <label className="text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Tipo de Rol</label>
                    <select
                      value={assignRoleType}
                      onChange={(e) => setAssignRoleType(e.target.value as any)}
                      className="bg-[#0A0A0A] border-2 border-white/10 text-xs font-mono font-black text-white rounded-none px-4 py-3.5 outline-none focus:border-white transition-colors cursor-pointer uppercase"
                    >
                      <option value="level">Nivel Faceit (1-10)</option>
                      <option value="special">Rol Especial del Club</option>
                    </select>
                  </div>

                  <div className="flex flex-col gap-1.5">
                    <label className="text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Valor o Nombre del Rol</label>
                    <input
                      type="text"
                      placeholder={assignRoleType === 'level' ? 'Ej: 10' : 'Ej: MASTER, EXTREME, GUEST'}
                      value={assignRoleValue}
                      onChange={(e) => setAssignRoleValue(e.target.value)}
                      className="bg-[#0A0A0A] border-2 border-white/10 text-xs font-mono font-black text-white rounded-none px-4 py-3.5 outline-none focus:border-white transition-colors uppercase"
                      required
                    />
                  </div>

                  <div className="pt-4 flex gap-3">
                    <button
                      type="button"
                      onClick={() => setRoleModalOpen(false)}
                      className="flex-1 py-4 px-4 text-xs font-black bg-[#1A1A1A] hover:bg-neutral-800 text-white transition-colors cursor-pointer rounded-none border border-white/10 uppercase tracking-tighter"
                    >
                      Cancelar
                    </button>
                    <button
                      type="submit"
                      className="flex-1 py-4 px-4 text-xs font-black bg-white hover:bg-neutral-200 text-black transition-colors cursor-pointer rounded-none border border-white uppercase tracking-tighter"
                    >
                      Asignar Rol
                    </button>
                  </div>

                </form>
              </div>
            </div>
          )}

          {/* Modal 3: ASIGNAR PERMISOS ADMINISTRATIVOS WEB */}
          {adminRoleModalOpen && adminRoleTargetUser && (
            <div className="fixed inset-0 z-[1000] flex items-center justify-center p-4">
              <div onClick={() => setAdminRoleModalOpen(false)} className="absolute inset-0 bg-black/90 backdrop-blur-sm" />
              
              <div className="relative bg-[#111] border-2 border-white p-8 shadow-[10px_10px_0px_rgba(0,0,0,0.5)] rounded-none w-full max-w-md z-10 font-mono">
                <h3 className="text-lg font-black text-white mb-6 uppercase tracking-tighter italic flex items-center gap-2">
                  <span>🔑 GESTIONAR PERMISOS WEB</span>
                </h3>

                <form onSubmit={handleSaveAdminRoleSubmit} className="space-y-4">
                  <div className="text-xs text-neutral-400 mb-4">
                    Asignando rol para: <span className="text-white font-black">{adminRoleTargetUser.faceitNickname}</span> ({adminRoleTargetUser.discordId})
                  </div>
                  
                  <div className="flex flex-col gap-1.5">
                    <label className="text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Rol del Usuario</label>
                    <select
                      value={adminRoleValue}
                      onChange={(e) => setAdminRoleValue(e.target.value as any)}
                      className="bg-[#0A0A0A] border-2 border-white/10 text-xs font-mono font-black text-white rounded-none px-4 py-3.5 outline-none focus:border-white transition-colors cursor-pointer uppercase"
                    >
                      <option value="usuario">Miembro / Usuario Común</option>
                      <option value="admin">Administrador de Servidor</option>
                      {currentUser?.role === 'global_admin' && (
                        <option value="global_admin">Administrador Global</option>
                      )}
                    </select>
                  </div>

                  {adminRoleValue === 'admin' && (
                    <div className="flex flex-col gap-1.5 animate-fadeIn">
                      <label className="text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Servidor a Asignar</label>
                      <select
                        value={adminRoleServer}
                        onChange={(e) => setAdminRoleServer(e.target.value)}
                        className="bg-[#0A0A0A] border-2 border-white/10 text-xs font-mono font-black text-white rounded-none px-4 py-3.5 outline-none focus:border-white transition-colors cursor-pointer uppercase"
                        required
                        disabled={currentUser?.role === 'admin'}
                      >
                        {currentUser?.role === 'admin' ? (
                          <option value={currentUser.servidor}>{guilds.find(g => g.id === currentUser.servidor)?.name || 'Tu Servidor'}</option>
                        ) : (
                          <>
                            <option value="">-- Seleccionar Servidor --</option>
                            {guilds.map(g => (
                              <option key={g.id} value={g.id}>{g.name}</option>
                            ))}
                          </>
                        )}
                      </select>
                    </div>
                  )}

                  <div className="pt-4 flex gap-3">
                    <button
                      type="button"
                      onClick={() => setAdminRoleModalOpen(false)}
                      className="flex-1 py-4 px-4 text-xs font-black bg-[#1A1A1A] hover:bg-neutral-800 text-white transition-colors cursor-pointer rounded-none border border-white/10 uppercase tracking-tighter"
                    >
                      Cancelar
                    </button>
                    <button
                      type="submit"
                      className="flex-1 py-4 px-4 text-xs font-black bg-white hover:bg-neutral-200 text-black transition-colors cursor-pointer rounded-none border border-white uppercase tracking-tighter"
                    >
                      Guardar Permisos
                    </button>
                  </div>

                </form>
              </div>
            </div>
          )}

          {/* Modal 4: VER ESTADÍSTICAS DE LA PARTIDA */}
          {selectedMatchStatsModal && (
            <div className="fixed inset-0 z-[1000] flex items-center justify-center p-4">
              <div onClick={() => setSelectedMatchStatsModal(null)} className="absolute inset-0 bg-black/90 backdrop-blur-sm" />
              <div className="relative bg-[#111] border-2 border-white/10 w-full max-w-4xl max-h-[90vh] overflow-y-auto p-6 text-white font-mono shadow-[15px_15px_0px_rgba(0,0,0,0.8)] z-10 animate-fade-in space-y-6">
                
                {/* Header Modal */}
                <div className="flex items-start justify-between border-b border-white/10 pb-4">
                  <div>
                    <div className="flex items-center gap-2">
                      <Trophy className="w-5 h-5 text-[#FF5500]" />
                      <h3 className="text-lg font-black uppercase tracking-tight italic">
                        ESTADÍSTICAS DE LA PARTIDA
                      </h3>
                    </div>
                    <p className="text-xs text-neutral-400 font-mono mt-0.5">
                      ID: <span className="text-[#FF5500] font-bold">{selectedMatchStatsModal.id}</span> • {selectedMatchStatsModal.entity_custom_name || 'Club'}
                    </p>
                  </div>
                  <button
                    onClick={() => setSelectedMatchStatsModal(null)}
                    className="p-1 text-neutral-400 hover:text-white bg-white/5 hover:bg-white/10 transition-colors cursor-pointer"
                  >
                    <X className="w-5 h-5" />
                  </button>
                </div>

                {/* Match Overview Banner */}
                <div className="grid grid-cols-1 md:grid-cols-3 gap-4 bg-[#181818] p-4 border border-white/10">
                  <div className="text-center md:text-left">
                    <span className="text-[10px] text-neutral-500 uppercase font-bold block">MAPA & LUGAR</span>
                    <span className="text-sm font-black text-white uppercase">{selectedMatchStatsModal.map || 'DESCONOCIDO'}</span>
                    <span className="text-xs text-neutral-400 block uppercase">{selectedMatchStatsModal.location || 'DESCONOCIDO'}</span>
                  </div>

                  {/* Score Display */}
                  <div className="flex flex-col items-center justify-center border-y md:border-y-0 md:border-x border-white/10 py-2 md:py-0">
                    <span className="text-[10px] text-neutral-500 uppercase font-bold">MARCADOR FINAL</span>
                    <div className="flex items-center gap-3 text-xl font-black mt-0.5">
                      <span className={selectedMatchStatsModal.winner === 'faction1' ? 'text-emerald-400' : 'text-white'}>
                        {selectedMatchStatsModal.faction1_score ?? 0}
                      </span>
                      <span className="text-neutral-600 text-sm">VS</span>
                      <span className={selectedMatchStatsModal.winner === 'faction2' ? 'text-emerald-400' : 'text-white'}>
                        {selectedMatchStatsModal.faction2_score ?? 0}
                      </span>
                    </div>
                    <span className="text-[9px] text-emerald-400 uppercase font-bold mt-1">
                      {selectedMatchStatsModal.winner === 'faction1' ? `Ganador: ${selectedMatchStatsModal.faction1_name}` : (selectedMatchStatsModal.winner === 'faction2' ? `Ganador: ${selectedMatchStatsModal.faction2_name}` : 'Finalizada')}
                    </span>
                  </div>

                  <div className="text-center md:text-right flex flex-col justify-center items-center md:items-end">
                    <span className="text-[10px] text-neutral-500 uppercase font-bold block">ESTADO</span>
                    <span className="text-xs font-bold text-emerald-400 bg-emerald-500/10 px-2 py-0.5 border border-emerald-500/20 uppercase mt-0.5">
                      {selectedMatchStatsModal.status || 'FINALIZADA'}
                    </span>
                    <button
                      onClick={() => handleDownloadMatchDemo(selectedMatchStatsModal)}
                      className="mt-2 text-[10px] font-bold text-blue-400 hover:text-blue-300 flex items-center gap-1 underline cursor-pointer"
                    >
                      <Download className="w-3 h-3" /> Descargar Demo / Partida
                    </button>
                  </div>
                </div>

                {/* Loading state for detailed stats */}
                {loadingMatchStats && (
                  <div className="py-8 text-center text-neutral-400 text-xs animate-pulse font-mono">
                    Cargando estadísticas detalladas desde la API de Faceit...
                  </div>
                )}

                {/* Rosters / Player Stats tables */}
                <div className="space-y-6">
                  {/* Faction 1 */}
                  <div className="border border-white/10 bg-[#141414] overflow-hidden">
                    <div className="bg-neutral-900 px-4 py-2.5 border-b border-white/10 flex justify-between items-center">
                      <span className={`font-black text-xs uppercase ${selectedMatchStatsModal.winner === 'faction1' ? 'text-emerald-400' : 'text-white'}`}>
                        {selectedMatchStatsModal.faction1_name || 'EQUIPO A'} ({selectedMatchStatsModal.faction1_score ?? 0})
                      </span>
                      {selectedMatchStatsModal.winner === 'faction1' && (
                        <span className="text-[9px] bg-emerald-500/20 text-emerald-400 px-2 py-0.5 font-bold uppercase border border-emerald-500/30">
                          GANADOR
                        </span>
                      )}
                    </div>
                    <div className="overflow-x-auto">
                      <table className="w-full text-left text-xs">
                        <thead className="bg-neutral-900/50 text-neutral-400 text-[10px] uppercase font-bold border-b border-white/5">
                          <tr>
                            <th className="py-2 px-4">Jugador</th>
                            <th className="py-2 px-4 text-center">Kills</th>
                            <th className="py-2 px-4 text-center">Deaths</th>
                            <th className="py-2 px-4 text-center">K/D</th>
                            <th className="py-2 px-4 text-center">HS%</th>
                            <th className="py-2 px-4 text-center">MVP</th>
                          </tr>
                        </thead>
                        <tbody className="divide-y divide-white/5 font-mono text-[11px]">
                          {(selectedMatchStatsModal.faction1_roster || []).length === 0 ? (
                            <tr>
                              <td colSpan={6} className="py-3 px-4 text-neutral-500 text-center">Sin datos de jugadores registrados</td>
                            </tr>
                          ) : (
                            (selectedMatchStatsModal.faction1_roster || []).map((p: any, idx: number) => {
                              let pStats: any = null;
                              if (matchStatsDetails?.rounds?.[0]?.teams) {
                                const team = matchStatsDetails.rounds[0].teams.find((t: any) => t.team_id === 'faction1' || t.premade_id === 'faction1');
                                if (team && team.players) {
                                  pStats = team.players.find((pl: any) => pl.player_id === p.id || pl.nickname === p.nickname)?.player_stats;
                                }
                              }
                              const kills = pStats?.Kills ?? '-';
                              const deaths = pStats?.Deaths ?? '-';
                              const kd = pStats?.['K/D Ratio'] ?? (pStats && deaths > 0 ? (kills / deaths).toFixed(2) : '-');
                              const hs = pStats?.['Headshots %'] ? `${pStats['Headshots %']}%` : '-';
                              const mvps = pStats?.MVPs ?? '-';

                              return (
                                <tr key={p.id || idx} className="hover:bg-white/5">
                                  <td className="py-2.5 px-4 font-bold text-white flex items-center gap-2">
                                    <span className="text-neutral-400 font-mono text-[10px]">#{idx + 1}</span>
                                    {p.nickname}
                                  </td>
                                  <td className="py-2.5 px-4 text-center text-emerald-400 font-bold">{kills}</td>
                                  <td className="py-2.5 px-4 text-center text-red-400">{deaths}</td>
                                  <td className="py-2.5 px-4 text-center font-bold">{kd}</td>
                                  <td className="py-2.5 px-4 text-center text-neutral-400">{hs}</td>
                                  <td className="py-2.5 px-4 text-center text-yellow-400">{mvps}</td>
                                </tr>
                              );
                            })
                          )}
                        </tbody>
                      </table>
                    </div>
                  </div>

                  {/* Faction 2 */}
                  <div className="border border-white/10 bg-[#141414] overflow-hidden">
                    <div className="bg-neutral-900 px-4 py-2.5 border-b border-white/10 flex justify-between items-center">
                      <span className={`font-black text-xs uppercase ${selectedMatchStatsModal.winner === 'faction2' ? 'text-emerald-400' : 'text-white'}`}>
                        {selectedMatchStatsModal.faction2_name || 'EQUIPO B'} ({selectedMatchStatsModal.faction2_score ?? 0})
                      </span>
                      {selectedMatchStatsModal.winner === 'faction2' && (
                        <span className="text-[9px] bg-emerald-500/20 text-emerald-400 px-2 py-0.5 font-bold uppercase border border-emerald-500/30">
                          GANADOR
                        </span>
                      )}
                    </div>
                    <div className="overflow-x-auto">
                      <table className="w-full text-left text-xs">
                        <thead className="bg-neutral-900/50 text-neutral-400 text-[10px] uppercase font-bold border-b border-white/5">
                          <tr>
                            <th className="py-2 px-4">Jugador</th>
                            <th className="py-2 px-4 text-center">Kills</th>
                            <th className="py-2 px-4 text-center">Deaths</th>
                            <th className="py-2 px-4 text-center">K/D</th>
                            <th className="py-2 px-4 text-center">HS%</th>
                            <th className="py-2 px-4 text-center">MVP</th>
                          </tr>
                        </thead>
                        <tbody className="divide-y divide-white/5 font-mono text-[11px]">
                          {(selectedMatchStatsModal.faction2_roster || []).length === 0 ? (
                            <tr>
                              <td colSpan={6} className="py-3 px-4 text-neutral-500 text-center">Sin datos de jugadores registrados</td>
                            </tr>
                          ) : (
                            (selectedMatchStatsModal.faction2_roster || []).map((p: any, idx: number) => {
                              let pStats: any = null;
                              if (matchStatsDetails?.rounds?.[0]?.teams) {
                                const team = matchStatsDetails.rounds[0].teams.find((t: any) => t.team_id === 'faction2' || t.premade_id === 'faction2');
                                if (team && team.players) {
                                  pStats = team.players.find((pl: any) => pl.player_id === p.id || pl.nickname === p.nickname)?.player_stats;
                                }
                              }
                              const kills = pStats?.Kills ?? '-';
                              const deaths = pStats?.Deaths ?? '-';
                              const kd = pStats?.['K/D Ratio'] ?? (pStats && deaths > 0 ? (kills / deaths).toFixed(2) : '-');
                              const hs = pStats?.['Headshots %'] ? `${pStats['Headshots %']}%` : '-';
                              const mvps = pStats?.MVPs ?? '-';

                              return (
                                <tr key={p.id || idx} className="hover:bg-white/5">
                                  <td className="py-2.5 px-4 font-bold text-white flex items-center gap-2">
                                    <span className="text-neutral-400 font-mono text-[10px]">#{idx + 1}</span>
                                    {p.nickname}
                                  </td>
                                  <td className="py-2.5 px-4 text-center text-emerald-400 font-bold">{kills}</td>
                                  <td className="py-2.5 px-4 text-center text-red-400">{deaths}</td>
                                  <td className="py-2.5 px-4 text-center font-bold">{kd}</td>
                                  <td className="py-2.5 px-4 text-center text-neutral-400">{hs}</td>
                                  <td className="py-2.5 px-4 text-center text-yellow-400">{mvps}</td>
                                </tr>
                              );
                            })
                          )}
                        </tbody>
                      </table>
                    </div>
                  </div>
                </div>

                {/* Modal Footer */}
                <div className="flex justify-between items-center border-t border-white/10 pt-4 text-xs font-mono">
                  {!selectedMatchStatsModal.id.startsWith('mixton_') && !selectedMatchStatsModal.id.startsWith('manual_') && selectedMatchStatsModal.launch_mode !== 'steam' && (
                    <a
                      href={`https://www.faceit.com/en/cs2/room/${selectedMatchStatsModal.id}`}
                      target="_blank"
                      rel="noreferrer"
                      className="text-[#FF5500] hover:underline flex items-center gap-1 font-bold"
                    >
                      <ExternalLink className="w-3.5 h-3.5" /> Abrir Sala en Faceit.com
                    </a>
                  )}
                  <button
                    onClick={() => setSelectedMatchStatsModal(null)}
                    className="ml-auto px-4 py-2 bg-[#222] hover:bg-white hover:text-black font-bold uppercase transition-colors cursor-pointer"
                  >
                    Cerrar
                  </button>
                </div>

              </div>
            </div>
          )}

          {/* Modal 5: CONFIRMAR ELIMINAR PARTIDA DEL HISTORIAL */}
          {matchToDelete && (
            <div className="fixed inset-0 z-[1000] flex items-center justify-center p-4">
              <div onClick={() => setMatchToDelete(null)} className="absolute inset-0 bg-black/90 backdrop-blur-sm" />
              <div className="relative bg-[#111] border-2 border-red-500/50 w-full max-w-md p-6 text-white font-mono shadow-[15px_15px_0px_rgba(0,0,0,0.8)] z-10 animate-fade-in space-y-4">
                <div className="flex items-center gap-3 text-red-400">
                  <ShieldAlert className="w-6 h-6" />
                  <h3 className="text-base font-black uppercase tracking-tight italic">
                    ELIMINAR PARTIDA DEL HISTORIAL
                  </h3>
                </div>

                <p className="text-xs text-neutral-300 leading-relaxed font-mono">
                  ¿Estás seguro de que deseas eliminar permanentemente la partida <span className="text-red-400 font-bold">{matchToDelete.id}</span> del historial de Firestore?
                </p>

                <div className="bg-[#1A1A1A] p-3 border border-white/10 text-[11px] space-y-1 text-neutral-400 font-mono">
                  <div><strong className="text-white">Enfrentamiento:</strong> {matchToDelete.faction1_name} vs {matchToDelete.faction2_name}</div>
                  <div><strong className="text-white">Mapa:</strong> {matchToDelete.map || 'N/A'}</div>
                  <div><strong className="text-white">Marcador:</strong> {matchToDelete.faction1_score} - {matchToDelete.faction2_score}</div>
                </div>

                <div className="flex justify-end gap-3 pt-2">
                  <button
                    onClick={() => setMatchToDelete(null)}
                    disabled={!!deletingMatchId}
                    className="px-4 py-2 bg-[#222] hover:bg-white hover:text-black text-xs font-bold uppercase transition-colors cursor-pointer"
                  >
                    Cancelar
                  </button>
                  <button
                    onClick={handleDeleteMatchHistory}
                    disabled={!!deletingMatchId}
                    className="px-4 py-2 bg-red-600 hover:bg-red-500 text-white text-xs font-black uppercase transition-colors cursor-pointer flex items-center gap-1.5"
                  >
                    <Trash2 className="w-3.5 h-3.5" />
                    {deletingMatchId ? 'Eliminando...' : 'Eliminar Partida'}
                  </button>
                </div>
              </div>
            </div>
          )}

          {/* Modal 6: GESTIONAR SALDO DE BILLETERA */}
          {balanceModalOpen && balanceTargetUser && (
            <div className="fixed inset-0 z-[1000] flex items-center justify-center p-4">
              <div onClick={() => setBalanceModalOpen(false)} className="absolute inset-0 bg-black/90 backdrop-blur-sm" />
              <div className="relative bg-[#111] border-2 border-emerald-500/50 w-full max-w-md p-6 text-white font-mono shadow-[15px_15px_0px_rgba(0,0,0,0.8)] z-10 animate-fade-in space-y-4">
                <div className="flex items-center justify-between border-b border-white/10 pb-3">
                  <div className="flex items-center gap-2">
                    <Wallet className="w-5 h-5 text-emerald-400" />
                    <h3 className="text-sm font-black uppercase tracking-tight italic">
                      GESTIÓN DE SALDO DE BILLETERA
                    </h3>
                  </div>
                  <button onClick={() => setBalanceModalOpen(false)} className="text-neutral-400 hover:text-white cursor-pointer">
                    <X className="w-4 h-4" />
                  </button>
                </div>

                <div className="bg-[#181818] p-3 border border-white/10 space-y-1 text-xs">
                  <div><span className="text-neutral-400">Usuario:</span> <strong className="text-white">{balanceTargetUser.faceitNickname}</strong></div>
                  <div><span className="text-neutral-400">Discord ID:</span> <span className="text-neutral-300">{balanceTargetUser.discordId}</span></div>
                  <div><span className="text-neutral-400">Saldo Actual:</span> <strong className="text-emerald-400 text-sm">S/ {(balanceTargetUser.wallet_balance || 0).toFixed(2)}</strong></div>
                </div>

                <form onSubmit={handleSaveBalanceSubmit} className="space-y-4 pt-1">
                  <div className="flex flex-col gap-1.5">
                    <label className="text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Tipo de Acción</label>
                    <select
                      value={balanceActionType}
                      onChange={(e) => setBalanceActionType(e.target.value as any)}
                      className="bg-[#0A0A0A] border-2 border-white/10 text-xs font-mono font-black text-white rounded-none px-4 py-3 outline-none focus:border-white transition-colors cursor-pointer uppercase"
                    >
                      <option value="add">➕ Añadir Fondos (+ S/.)</option>
                      <option value="subtract">➖ Descontar Fondos (- S/.)</option>
                      <option value="set">🎯 Establecer Saldo Exacto (= S/.)</option>
                    </select>
                  </div>

                  <div className="flex flex-col gap-1.5">
                    <label className="text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Monto en Soles (S/.)</label>
                    <input
                      type="number"
                      step="0.01"
                      min="0"
                      placeholder="0.00"
                      value={balanceAmount}
                      onChange={(e) => setBalanceAmount(e.target.value)}
                      className="bg-[#0A0A0A] border-2 border-white/10 text-xs font-mono font-black text-white rounded-none px-4 py-3 outline-none focus:border-white transition-colors"
                      required
                    />
                  </div>

                  <div className="flex flex-col gap-1.5">
                    <label className="text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Motivo / Razón de Auditoría</label>
                    <input
                      type="text"
                      placeholder="Ej: Recarga por depósito manual / Corrección de saldo"
                      value={balanceReason}
                      onChange={(e) => setBalanceReason(e.target.value)}
                      className="bg-[#0A0A0A] border-2 border-white/10 text-xs font-mono text-white rounded-none px-4 py-3 outline-none focus:border-white transition-colors"
                      required
                    />
                  </div>

                  <div className="pt-2 flex gap-3">
                    <button
                      type="button"
                      onClick={() => setBalanceModalOpen(false)}
                      className="flex-1 py-3 px-4 text-xs font-black bg-[#1A1A1A] hover:bg-neutral-800 text-white transition-colors cursor-pointer rounded-none border border-white/10 uppercase"
                    >
                      Cancelar
                    </button>
                    <button
                      type="submit"
                      disabled={savingBalance}
                      className="flex-1 py-3 px-4 text-xs font-black bg-emerald-500 hover:bg-emerald-400 text-black transition-colors cursor-pointer rounded-none border border-emerald-400 uppercase font-bold"
                    >
                      {savingBalance ? 'Guardando...' : 'Aplicar Saldo'}
                    </button>
                  </div>
                </form>
              </div>
            </div>
          )}

          {/* Modal 7: DETALLE Y RESPUESTA DE RECLAMACIÓN INDECOPI */}
          {selectedClaimModal && (
            <div className="fixed inset-0 z-[1000] flex items-center justify-center p-4">
              <div onClick={() => setSelectedClaimModal(null)} className="absolute inset-0 bg-black/90 backdrop-blur-sm" />
              <div className="relative bg-[#111] border-2 border-amber-500/50 w-full max-w-2xl max-h-[90vh] overflow-y-auto p-6 text-white font-mono shadow-[15px_15px_0px_rgba(0,0,0,0.8)] z-10 animate-fade-in space-y-4">
                <div className="flex items-center justify-between border-b border-white/10 pb-3">
                  <div className="flex items-center gap-2">
                    <Scale className="w-5 h-5 text-amber-400" />
                    <div>
                      <h3 className="text-sm font-black uppercase tracking-tight italic">
                        HOJA DE RECLAMACIÓN: {selectedClaimModal.claimCode || selectedClaimModal.id}
                      </h3>
                      <span className="text-[10px] text-neutral-400">Ley Nº 29571 - Código de Protección al Consumidor</span>
                    </div>
                  </div>
                  <button onClick={() => setSelectedClaimModal(null)} className="text-neutral-400 hover:text-white cursor-pointer">
                    <X className="w-4 h-4" />
                  </button>
                </div>

                {/* Consumer Details */}
                <div className="grid grid-cols-1 md:grid-cols-2 gap-3 bg-[#161616] p-4 border border-white/10 text-xs">
                  <div>
                    <span className="text-[10px] text-neutral-500 font-bold uppercase block">1. IDENTIFICACIÓN DEL CONSUMIDOR</span>
                    <div className="text-white font-bold mt-1">{selectedClaimModal.consumer?.name || 'N/A'}</div>
                    <div className="text-neutral-400">{selectedClaimModal.consumer?.docType || 'DNI'}: {selectedClaimModal.consumer?.docNumber || '-'}</div>
                    <div className="text-neutral-400">Email: {selectedClaimModal.consumer?.email || '-'}</div>
                    <div className="text-neutral-400">Teléfono: {selectedClaimModal.consumer?.phone || '-'}</div>
                    <div className="text-neutral-500 text-[11px] mt-0.5">Dir: {selectedClaimModal.consumer?.address || '-'}</div>
                  </div>
                  <div>
                    <span className="text-[10px] text-neutral-500 font-bold uppercase block">2. BIEN CONTRATADO</span>
                    <div className="text-white font-bold mt-1">{selectedClaimModal.goods?.type || 'PRODUCTO'} - S/ {(selectedClaimModal.goods?.amount || 0).toFixed(2)}</div>
                    <div className="text-neutral-400 text-[11px] mt-1">{selectedClaimModal.goods?.description || 'Sin descripción'}</div>
                    <div className="mt-2 text-[10px] text-neutral-500">
                      Fecha: {formatDateDisplay(selectedClaimModal.createdAtIso || selectedClaimModal.createdAt)}
                    </div>
                  </div>
                </div>

                {/* Claim Body */}
                <div className="bg-[#181818] p-4 border border-white/10 space-y-3 text-xs">
                  <div className="flex items-center gap-2">
                    <span className="text-[10px] font-bold text-neutral-400 uppercase">TIPO:</span>
                    <span className={`px-2 py-0.5 text-[9px] font-bold uppercase border ${
                      selectedClaimModal.claim?.type === 'QUEJA' ? 'bg-purple-950/40 text-purple-300 border-purple-500/30' : 'bg-amber-950/40 text-amber-300 border-amber-500/30'
                    }`}>
                      {selectedClaimModal.claim?.type || 'RECLAMO'}
                    </span>
                  </div>

                  <div>
                    <span className="text-[10px] text-neutral-500 font-bold uppercase block">DETALLE DEL RECLAMO:</span>
                    <p className="text-neutral-200 mt-1 whitespace-pre-wrap bg-[#111] p-3 border border-white/5 text-[11px] leading-relaxed">
                      {selectedClaimModal.claim?.detail || 'Sin detalle provisto.'}
                    </p>
                  </div>

                  <div>
                    <span className="text-[10px] text-neutral-500 font-bold uppercase block">PEDIDO CONCRETO DEL CONSUMIDOR:</span>
                    <p className="text-neutral-200 mt-1 whitespace-pre-wrap bg-[#111] p-3 border border-white/5 text-[11px] leading-relaxed">
                      {selectedClaimModal.claim?.request || 'Sin pedido específico.'}
                    </p>
                  </div>
                </div>

                {/* Current Response or Form */}
                {selectedClaimModal.response && (
                  <div className="bg-emerald-950/20 border border-emerald-500/30 p-4 space-y-1">
                    <span className="text-[10px] font-bold text-emerald-400 uppercase block">RESPUESTA REGISTRADA PREVIAMENTE:</span>
                    <p className="text-xs text-neutral-200 whitespace-pre-wrap">{selectedClaimModal.response}</p>
                    <div className="text-[10px] text-neutral-500 mt-2">
                      Por: {selectedClaimModal.respondedBy || 'Admin'} | Fecha: {formatDateDisplay(selectedClaimModal.respondedAtIso || selectedClaimModal.respondedAt)}
                    </div>
                  </div>
                )}

                <form onSubmit={handleRespondClaimSubmit} className="space-y-3 pt-2 border-t border-white/10">
                  <span className="text-xs font-black uppercase text-white block">RESPONDER O ACTUALIZAR ESTADO</span>
                  
                  <div className="flex flex-col gap-1.5">
                    <label className="text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Estado de la Reclamación</label>
                    <select
                      value={claimResponseStatus}
                      onChange={(e) => setClaimResponseStatus(e.target.value as any)}
                      className="bg-[#0A0A0A] border-2 border-white/10 text-xs font-mono font-black text-white rounded-none px-4 py-3 outline-none focus:border-white transition-colors cursor-pointer uppercase"
                    >
                      <option value="ATENDIDO">ATENDIDO (Conforme o Solución Emitida)</option>
                      <option value="EN_PROCESO">EN PROCESO (Investigación en curso)</option>
                      <option value="RECHAZADO">RECHAZADO (Improcedente)</option>
                    </select>
                  </div>

                  <div className="flex flex-col gap-1.5">
                    <label className="text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Texto de Respuesta Oficial Formal</label>
                    <textarea
                      rows={4}
                      placeholder="Escriba aquí la respuesta formal y fundamentada que se notificará al consumidor..."
                      value={claimResponseText}
                      onChange={(e) => setClaimResponseText(e.target.value)}
                      className="bg-[#0A0A0A] border-2 border-white/10 text-xs font-mono text-white rounded-none p-3 outline-none focus:border-white transition-colors"
                      required
                    />
                  </div>

                  <div className="flex justify-end gap-3 pt-2">
                    <button
                      type="button"
                      onClick={() => setSelectedClaimModal(null)}
                      className="px-4 py-2.5 bg-[#222] hover:bg-neutral-800 text-xs font-bold uppercase transition-colors cursor-pointer"
                    >
                      Cerrar
                    </button>
                    <button
                      type="submit"
                      disabled={respondingClaim}
                      className="px-5 py-2.5 bg-amber-500 hover:bg-amber-400 text-black text-xs font-black uppercase tracking-wider transition-colors cursor-pointer"
                    >
                      {respondingClaim ? 'Guardando...' : 'Enviar Respuesta Oficial'}
                    </button>
                  </div>
                </form>
              </div>
            </div>
          )}

          {/* Modal 8: DETALLE DE ÓRDEN DE COMPRA */}
          {selectedOrderModal && (
            <div className="fixed inset-0 z-[1000] flex items-center justify-center p-4">
              <div onClick={() => setSelectedOrderModal(null)} className="absolute inset-0 bg-black/90 backdrop-blur-sm" />
              <div className="relative bg-[#111] border-2 border-white/20 w-full max-w-xl max-h-[90vh] overflow-y-auto p-6 text-white font-mono shadow-[15px_15px_0px_rgba(0,0,0,0.8)] z-10 animate-fade-in space-y-4">
                <div className="flex items-center justify-between border-b border-white/10 pb-3">
                  <div className="flex items-center gap-2">
                    <ShoppingBag className="w-5 h-5 text-white" />
                    <h3 className="text-sm font-black uppercase tracking-tight italic">
                      DETALLE DE ÓRDEN: {selectedOrderModal.orderId || selectedOrderModal.id}
                    </h3>
                  </div>
                  <button onClick={() => setSelectedOrderModal(null)} className="text-neutral-400 hover:text-white cursor-pointer">
                    <X className="w-4 h-4" />
                  </button>
                </div>

                <div className="bg-[#181818] p-4 border border-white/10 space-y-2 text-xs">
                  <div><span className="text-neutral-400">Cliente:</span> <strong className="text-white uppercase">{selectedOrderModal.customer?.name || 'Cliente'}</strong></div>
                  <div><span className="text-neutral-400">Discord ID:</span> <span className="text-neutral-200">{selectedOrderModal.customer?.discordId || 'N/A'}</span></div>
                  <div><span className="text-neutral-400">Email:</span> <span className="text-neutral-200">{selectedOrderModal.customer?.email || 'N/A'}</span></div>
                  <div><span className="text-neutral-400">Teléfono:</span> <span className="text-neutral-200">{selectedOrderModal.customer?.phone || 'N/A'}</span></div>
                  <div><span className="text-neutral-400">Total:</span> <strong className="text-emerald-400 text-sm">S/ {(selectedOrderModal.totalAmount || (selectedOrderModal as any).total || 0).toFixed(2)}</strong></div>
                  <div><span className="text-neutral-400">Referencia:</span> <span className="text-purple-400">{selectedOrderModal.referenceCode || selectedOrderModal.chargeId || '-'}</span></div>
                </div>

                <div className="space-y-2">
                  <span className="text-[10px] font-bold text-neutral-400 uppercase block">PRODUCTOS ADQUIRIDOS:</span>
                  <div className="bg-[#141414] border border-white/10 p-3 space-y-2 max-h-40 overflow-y-auto">
                    {(selectedOrderModal.items || []).map((it, idx) => (
                      <div key={idx} className="flex justify-between items-center text-xs border-b border-white/5 pb-1">
                        <div>
                          <span className="text-white font-bold">{it.quantity}x</span> <span className="text-neutral-300">{it.name}</span>
                        </div>
                        <div className="text-emerald-400 font-mono">
                          S/ {(it.itemTotal || (it.unitPrice * it.quantity) || 0).toFixed(2)}
                        </div>
                      </div>
                    ))}
                  </div>
                </div>

                {selectedOrderModal.adminNote && (
                  <div className="bg-neutral-900 p-3 border border-white/10 text-xs">
                    <span className="text-[10px] text-neutral-400 uppercase font-bold block">NOTA ADMINISTRATIVA:</span>
                    <p className="text-neutral-300 mt-0.5">{selectedOrderModal.adminNote}</p>
                  </div>
                )}

                <div className="border-t border-white/10 pt-4 space-y-3">
                  <span className="text-xs font-black uppercase text-white block">CAMBIAR ESTADO DE LA ORDEN</span>
                  <div className="grid grid-cols-2 gap-2">
                    <button
                      onClick={() => handleUpdateOrderStatus(selectedOrderModal.orderId || selectedOrderModal.id, 'PENDIENTE', 'Pendiente de entrega')}
                      disabled={updatingOrderStatus}
                      className="py-2.5 px-3 bg-yellow-950/20 text-yellow-400 hover:bg-yellow-600 hover:text-white border border-yellow-500/30 text-xs font-black uppercase transition-colors cursor-pointer"
                    >
                      Pendiente
                    </button>
                    <button
                      onClick={() => handleUpdateOrderStatus(selectedOrderModal.orderId || selectedOrderModal.id, 'PAGADO', 'Pago acreditado')}
                      disabled={updatingOrderStatus}
                      className="py-2.5 px-3 bg-blue-950/20 text-blue-400 hover:bg-blue-600 hover:text-white border border-blue-500/30 text-xs font-black uppercase transition-colors cursor-pointer"
                    >
                      Pagado
                    </button>
                    <button
                      onClick={() => handleUpdateOrderStatus(selectedOrderModal.orderId || selectedOrderModal.id, 'ENTREGADO', 'Item enviado/entregado al usuario')}
                      disabled={updatingOrderStatus}
                      className="py-2.5 px-3 bg-emerald-950/20 text-emerald-400 hover:bg-emerald-600 hover:text-white border border-emerald-500/30 text-xs font-black uppercase transition-colors cursor-pointer"
                    >
                      Entregado
                    </button>
                    <button
                      onClick={() => handleUpdateOrderStatus(selectedOrderModal.orderId || selectedOrderModal.id, 'CANCELADO', 'Orden cancelada')}
                      disabled={updatingOrderStatus}
                      className="py-2.5 px-3 bg-red-950/20 text-red-400 hover:bg-red-600 hover:text-white border border-red-500/30 text-xs font-black uppercase transition-colors cursor-pointer"
                    >
                      Cancelado
                    </button>
                  </div>
                </div>

                <div className="flex justify-end pt-2">
                  <button
                    onClick={() => setSelectedOrderModal(null)}
                    className="px-5 py-2.5 bg-[#222] hover:bg-white hover:text-black text-xs font-bold uppercase transition-colors cursor-pointer"
                  >
                    Cerrar
                  </button>
                </div>
              </div>
            </div>
          )}

          {/* Modal 9: APROBACIÓN MANUAL DE DEPÓSITO YAPE */}
          {manualApproveModalOpen && (
            <div className="fixed inset-0 z-[1000] flex items-center justify-center p-4">
              <div onClick={() => setManualApproveModalOpen(false)} className="absolute inset-0 bg-black/90 backdrop-blur-sm" />
              <div className="relative bg-[#111] border-2 border-emerald-500/50 w-full max-w-md p-6 text-white font-mono shadow-[15px_15px_0px_rgba(0,0,0,0.8)] z-10 animate-fade-in space-y-4">
                <div className="flex items-center justify-between border-b border-white/10 pb-3">
                  <div className="flex items-center gap-2">
                    <CheckCircle className="w-5 h-5 text-emerald-400" />
                    <h3 className="text-sm font-black uppercase tracking-tight italic">
                      APROBACIÓN MANUAL DE DEPÓSITO
                    </h3>
                  </div>
                  <button onClick={() => setManualApproveModalOpen(false)} className="text-neutral-400 hover:text-white cursor-pointer">
                    <X className="w-4 h-4" />
                  </button>
                </div>

                <p className="text-xs text-neutral-300">
                  Acredita directamente saldo a la billetera de un usuario validando un comprobante o constancia de Yape.
                </p>

                <form onSubmit={handleManualApproveSubmit} className="space-y-4 pt-1">
                  <div className="flex flex-col gap-1.5">
                    <label className="text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Discord ID del Usuario</label>
                    <input
                      type="text"
                      placeholder="Ej: 1982736451829384"
                      value={manualApproveDiscordId}
                      onChange={(e) => setManualApproveDiscordId(e.target.value)}
                      className="bg-[#0A0A0A] border-2 border-white/10 text-xs font-mono font-bold text-white rounded-none px-4 py-3 outline-none focus:border-white transition-colors"
                      required
                    />
                  </div>

                  <div className="flex flex-col gap-1.5">
                    <label className="text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">N° de Operación Yape / Token</label>
                    <input
                      type="text"
                      placeholder="Ej: 1234567 o RES-ABC-123"
                      value={manualApproveOp}
                      onChange={(e) => setManualApproveOp(e.target.value)}
                      className="bg-[#0A0A0A] border-2 border-white/10 text-xs font-mono font-bold text-white rounded-none px-4 py-3 outline-none focus:border-white transition-colors"
                      required
                    />
                  </div>

                  <div className="flex flex-col gap-1.5">
                    <label className="text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Monto Acreditado (S/.)</label>
                    <input
                      type="number"
                      step="0.01"
                      min="0.10"
                      placeholder="10.00"
                      value={manualApproveAmount}
                      onChange={(e) => setManualApproveAmount(e.target.value)}
                      className="bg-[#0A0A0A] border-2 border-white/10 text-xs font-mono font-bold text-white rounded-none px-4 py-3 outline-none focus:border-white transition-colors"
                      required
                    />
                  </div>

                  <div className="flex flex-col gap-1.5">
                    <label className="text-[10px] font-mono font-black uppercase tracking-widest text-neutral-400">Motivo / Comprobante</label>
                    <input
                      type="text"
                      placeholder="Ej: Constancia verificada en banca móvil"
                      value={manualApproveReason}
                      onChange={(e) => setManualApproveReason(e.target.value)}
                      className="bg-[#0A0A0A] border-2 border-white/10 text-xs font-mono text-white rounded-none px-4 py-3 outline-none focus:border-white transition-colors"
                      required
                    />
                  </div>

                  <div className="pt-2 flex gap-3">
                    <button
                      type="button"
                      onClick={() => setManualApproveModalOpen(false)}
                      className="flex-1 py-3 px-4 text-xs font-black bg-[#1A1A1A] hover:bg-neutral-800 text-white transition-colors cursor-pointer rounded-none border border-white/10 uppercase"
                    >
                      Cancelar
                    </button>
                    <button
                      type="submit"
                      disabled={approvingYape}
                      className="flex-1 py-3 px-4 text-xs font-black bg-emerald-500 hover:bg-emerald-400 text-black transition-colors cursor-pointer rounded-none border border-emerald-400 uppercase font-bold"
                    >
                      {approvingYape ? 'Aprobando...' : 'Aprobar y Acreditar'}
                    </button>
                  </div>
                </form>
              </div>
            </div>
          )}

        </div>
      )}

    </div>
  );
}

