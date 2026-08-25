import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { managerGet, managerPost } from './bridge.js';

const SECTIONS = [
  ['dashboard', 'Overview', 'Live control-plane summary'],
  ['setup', 'Telegram', 'Owner binding and bot transport'],
  ['providers', 'Custom AI', 'Profiles, probes, and write-only credentials'],
  ['agent', 'Agent', 'Loop, streaming, execution and latency controls'],
  ['sessions', 'Sessions', 'Conversation state and exact AI selection'],
  ['attachments', 'Attachments', 'Quota, ingestion, and processing records'],
  ['tasks', 'Runs', 'Observable work and cancellation'],
  ['memory', 'Memory', 'Owner-managed durable state'],
  ['skills', 'Skills', 'Learned and imported procedures'],
  ['tools', 'Tools', 'Typed capability surface'],
  ['security', 'Security', 'Approvals, YOLO, and audit'],
  ['runtime', 'Runtime', 'Device truth and execution environment'],
  ['diagnostics', 'Diagnostics', 'Independent health checks'],
  ['logs', 'Logs', 'Redacted daemon trace']
];

const SECTION_MAP = Object.fromEntries(SECTIONS.map(section => [section[0], section]));
const VIEW_RESOURCE = { setup: 'telegram' };

function resourceForView(view) {
  return VIEW_RESOURCE[view] || view;
}

const ICONS = {
  dashboard: 'M3 3h7v7H3V3Zm11 0h7v7h-7V3ZM3 14h7v7H3v-7Zm11 0h7v7h-7v-7Z',
  setup: 'M12 3a3 3 0 0 0-2.83 4H4v3h5.17a3 3 0 0 0 0 4H4v3h5.17A3 3 0 1 0 12 14a3 3 0 0 0 0-4h8V7h-8a3 3 0 0 0 0-4Zm0 3a1 1 0 1 1 0 2 1 1 0 0 1 0-2Zm0 8a1 1 0 1 1 0 2 1 1 0 0 1 0-2Z',
  providers: 'M12 2 3 7v10l9 5 9-5V7l-9-5Zm0 3.1L17 8l-5 2.9L7 8l5-2.9Zm-6 5.5 5 2.9v5.5l-5-2.8v-5.6Zm7 8.4v-5.5l5-2.9v5.6l-5 2.8Z',
  agent: 'M12 2a4 4 0 0 0-4 4v1H6a3 3 0 0 0-3 3v7a3 3 0 0 0 3 3h12a3 3 0 0 0 3-3v-7a3 3 0 0 0-3-3h-2V6a4 4 0 0 0-4-4Zm-2 5V6a2 2 0 1 1 4 0v1h-4Z',
  sessions: 'M4 4h16v12H7l-3 3V4Zm3 4h10v2H7V8Zm0 4h7v2H7v-2Z',
  attachments: 'M8 3h8l4 4v14H4V3h4Zm7 1.5V8h3.5L15 4.5ZM8 12h8v2H8v-2Zm0 4h8v2H8v-2Z',
  tasks: 'M13 2 4 14h7l-1 8 10-13h-7l0-7Z',
  memory: 'M5 4h14v16H5V4Zm3 3v10h8V7H8Zm2 2h4v2h-4V9Zm0 4h4v2h-4v-2Z',
  skills: 'm12 3 2.1 4.8L19 8.2l-3.7 3.5.9 5.1-4.5-2.5-4.5 2.5.9-5.1L5 8.2l4.9-.4L12 3Z',
  tools: 'M13.6 3.2a5.3 5.3 0 0 0 6.5 6.5l-4.3 4.3-2.2-2.2-8 8-2.4-2.4 8-8-2.2-2.2 4.6-3.9Zm5.2 2.3-1.7 1.7 1.1 1.1 1.7-1.7a3.2 3.2 0 0 1-1.1-1.1Z',
  security: 'M12 2 4 5v6c0 5.1 3.4 9.7 8 11 4.6-1.3 8-5.9 8-11V5l-8-3Zm0 4 4 1.5V11c0 3.2-1.8 6.2-4 7.4-2.2-1.2-4-4.2-4-7.4V7.5L12 6Zm-1 3v4h2V9h-2Zm0 5v2h2v-2h-2Z',
  runtime: 'M5 4h14v16H5V4Zm3 3v2h8V7H8Zm0 4v2h5v-2H8Zm0 4v2h8v-2H8Z',
  diagnostics: 'M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18Zm0 3a1.3 1.3 0 1 1 0 2.6A1.3 1.3 0 0 1 12 6Zm-1 5h2v6h-2v-6Z',
  logs: 'M5 5h14v2H5V5Zm0 5h14v2H5v-2Zm0 5h14v2H5v-2Z',
  refresh: 'M18.6 8A7 7 0 1 0 19 14h-2a5 5 0 1 1-1.4-3.5L13 13h7V6l-1.4 2Z',
  previous: 'm14.5 5-7 7 7 7 1.5-1.5L10.5 12 16 6.5 14.5 5Z',
  next: 'm9.5 5-1.5 1.5 5.5 5.5L8 17.5 9.5 19l7-7-7-7Z'
};

function Icon({ name, label }) {
  return <svg className="icon" viewBox="0 0 24 24" aria-hidden={label ? undefined : 'true'} role={label ? 'img' : undefined} aria-label={label} focusable="false"><path d={ICONS[name] || ICONS.tools} /></svg>;
}

function formatDate(value) {
  if (!value) return '—';
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? String(value) : date.toLocaleString();
}

function formatBytes(value) {
  const bytes = Number(value || 0);
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
}

function titleCase(value) {
  return String(value || 'unknown').replaceAll('_', ' ');
}

function classForStatus(value) {
  const status = String(value || 'unknown').toLowerCase().replaceAll(' ', '_');
  if (['ready', 'reachable', 'completed', 'available', 'enabled', 'pass', 'supported', 'configured', 'running', 'verified_success'].includes(status)) return 'ok';
  if (['failed', 'unreachable', 'denied', 'forbidden', 'error', 'cancelled', 'blocked'].includes(status)) return 'bad';
  if (['warn', 'unknown', 'unprobed', 'indeterminate', 'awaiting_approval', 'approval_required', 'missing_installable'].includes(status)) return 'warn';
  return 'muted';
}

function modelReadiness(model) {
  const probe = String(model?.probe_status || 'unprobed').toLowerCase();
  if (probe === 'unprobed') return 'Probe required';
  if (probe === 'indeterminate') return 'Probe required (indeterminate)';
  if (model?.native_tools_state === 'supported' && model?.continuation_state === 'supported') return 'Native Agent';
  if (model?.native_tools_state === 'unsupported' && model?.structured_output_state === 'supported' && model?.continuation_state === 'supported') return 'Structured Agent';
  if (model?.native_tools_state === 'unsupported' && model?.structured_output_state === 'unsupported' && model?.continuation_state === 'unsupported') return 'Chat only';
  return 'Protocol indeterminate';
}

function requiresExactProbe(model) {
  return ['unprobed', 'indeterminate'].includes(String(model?.probe_status || 'unprobed').toLowerCase());
}

function committedWarnings(result) {
  const values = [
    result?.warning,
    result?.cleanup_warning,
    result?.cleanup_warnings,
    result?.result?.warning,
    result?.result?.cleanup_warning,
    result?.result?.cleanup_warnings
  ];
  return values.flatMap(value => Array.isArray(value) ? value : value ? [value] : [])
    .map(value => String(value).trim())
    .filter(Boolean)
    .slice(0, 3)
    .map(value => value.slice(0, 280));
}

function parseHeaderObject(value, kind) {
  const text = String(value || '').trim();
  if (!text) return undefined;
  const parsed = JSON.parse(text);
  if (!parsed || Array.isArray(parsed) || typeof parsed !== 'object') {
    throw new Error(`${kind} headers must be a JSON object.`);
  }
  for (const [name, headerValue] of Object.entries(parsed)) {
    if (!String(name).trim() || typeof headerValue !== 'string') {
      throw new Error(`${kind} header names and values must be non-empty strings.`);
    }
    if (kind === 'Safe' && /^(authorization|proxy-authorization|cookie|set-cookie|x-api-key)$/i.test(name.trim())) {
      throw new Error(`${name} is secret-bearing; put it in Secret headers instead.`);
    }
  }
  return parsed;
}

function App() {
  const [view, setView] = useState('dashboard');
  const [data, setData] = useState(null);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState({ tone: 'muted', text: 'Connecting to the local Xiao control plane…' });
  const [page, setPage] = useState({ sessions: 1, attachments: 1, tasks: 1, memory: 1, skills: 1 });
  const [filters, setFilters] = useState({ memory: '', skills: '', logLines: 250 });
  const [providers, setProviders] = useState(null);
  const [confirm, setConfirm] = useState(null);
  const [prompt, setPrompt] = useState(null);
  const [profileEditor, setProfileEditor] = useState(null);
  const [sessionEditor, setSessionEditor] = useState(null);
  const currentView = useRef(view);
  const requestSequence = useRef(0);

  useEffect(() => { currentView.current = view; }, [view]);

  const queryFor = useCallback((target) => {
    if (target === 'sessions') return { page: page.sessions, limit: 5, include_archived: false };
    if (target === 'attachments') return { page: page.attachments, limit: 25 };
    if (target === 'tasks') return { page: page.tasks, limit: 5 };
    if (target === 'memory') return { page: page.memory, limit: 5, query: filters.memory, scope: 'all' };
    if (target === 'skills') return { page: page.skills, limit: 5, query: filters.skills };
    if (target === 'logs') return { lines: filters.logLines };
    return {};
  }, [filters.logLines, filters.memory, filters.skills, page]);

  const refresh = useCallback(async (target = view, query = queryFor(target), quiet = false) => {
    const request = ++requestSequence.current;
    setBusy(true);
    try {
      const next = await managerGet(resourceForView(target), query);
      if (target === currentView.current && request === requestSequence.current) {
        setData(next);
        if (!quiet) setNotice({ tone: 'ok', text: `${SECTION_MAP[target]?.[1] || target} refreshed from xiaod.` });
      }
      return next;
    } catch (error) {
      if (target === currentView.current && request === requestSequence.current) {
        setNotice({ tone: 'bad', text: `Could not load ${SECTION_MAP[target]?.[1] || target}: ${error.message}` });
      }
      return null;
    } finally {
      if (request === requestSequence.current) setBusy(false);
    }
  }, [queryFor, view]);

  const refreshProviders = useCallback(async () => {
    try {
      const next = await managerGet('providers');
      setProviders(next);
      return next;
    } catch (error) {
      setNotice({ tone: 'bad', text: `Could not load Custom profiles: ${error.message}` });
      return null;
    }
  }, []);

  useEffect(() => { refresh(view); }, [view]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => { if (view === 'providers' || view === 'sessions') refreshProviders(); }, [view, refreshProviders]);

  const action = useCallback(async (resource, body, success, target = view) => {
    setBusy(true);
    try {
      const result = await managerPost(resource, body);
      const warnings = committedWarnings(result);
      setNotice(warnings.length
        ? { tone: 'warn', text: `${success} Committed with cleanup warning: ${warnings.join(' · ')}` }
        : { tone: 'ok', text: success });
      if (resource === 'provider-custom') await refreshProviders();
      await refresh(target, queryFor(target), true);
      return result;
    } catch (error) {
      setNotice({ tone: 'bad', text: `Action failed: ${error.message}` });
      return null;
    } finally {
      setBusy(false);
    }
  }, [queryFor, refresh, refreshProviders, view]);

  const navigate = target => {
    if (target === view) return;
    setData(null);
    setView(target);
  };

  const changePage = (target, next) => {
    setPage(current => ({ ...current, [target]: Math.max(1, next) }));
    setTimeout(() => refresh(target, { ...queryFor(target), page: Math.max(1, next) }, true), 0);
  };

  const useSessionAi = async ({ sessionId, profileId, model }) => {
    const profile = providers?.custom_profiles?.find(item => item.id === profileId);
    const selected = profile?.models?.find(item => item.model_id === model);
    setBusy(true);
    try {
      if (requiresExactProbe(selected)) {
        setNotice({ tone: 'warn', text: `Exact-probing ${model} before it can be activated…` });
        await managerPost('provider-custom', { action: 'probe', profile_id: profileId, model });
      }
      await managerPost('sessions', {
        action: 'ai_config', session_id: sessionId, provider: 'custom', account_or_profile_id: profileId, model
      });
      setNotice({ tone: 'ok', text: 'Custom profile and exact model updated for this session only.' });
      setSessionEditor(null);
      await refreshProviders();
      await refresh('sessions', queryFor('sessions'), true);
    } catch (error) {
      setNotice({ tone: 'bad', text: `Could not set session AI: ${error.message}` });
    } finally {
      setBusy(false);
    }
  };

  const content = useMemo(() => {
    const props = { data, busy, action, refresh, setConfirm, setPrompt, setProfileEditor, setSessionEditor, providers, refreshProviders, filters, setFilters, changePage, setNotice };
    switch (view) {
      case 'dashboard': return <DashboardView {...props} />;
      case 'setup': return <SetupView {...props} />;
      case 'providers': return <ProvidersView {...props} />;
      case 'agent': return <AgentView {...props} />;
      case 'sessions': return <SessionsView {...props} />;
      case 'attachments': return <AttachmentsView {...props} />;
      case 'tasks': return <TasksView {...props} />;
      case 'memory': return <MemoryView {...props} />;
      case 'skills': return <SkillsView {...props} />;
      case 'tools': return <ToolsView {...props} />;
      case 'security': return <SecurityView {...props} />;
      case 'runtime': return <RuntimeView {...props} />;
      case 'diagnostics': return <DiagnosticsView {...props} />;
      case 'logs': return <LogsView {...props} />;
      default: return null;
    }
  }, [action, busy, changePage, data, filters, providers, refresh, refreshProviders, view]);

  return <div className="app-shell">
    <aside className="sidebar" aria-label="Xiao Root Manager navigation">
      <div className="brand">
        <div className="brand-mark" aria-hidden="true">肖</div>
        <div><span>ROOT CONTROL PLANE</span><strong>Xiao Manager</strong></div>
      </div>
      <label className="compact-nav"><span>Manager section</span><select value={view} onChange={event => navigate(event.target.value)} aria-label="Manager section">{SECTIONS.map(([id, label]) => <option key={id} value={id}>{label}</option>)}</select></label>
      <nav className="nav-list">
        {SECTIONS.map(([id, label, description]) => <button key={id} type="button" className={view === id ? 'nav-item selected' : 'nav-item'} onClick={() => navigate(id)} aria-current={view === id ? 'page' : undefined} title={description}>
          <Icon name={id} /><span>{label}</span>
        </button>)}
      </nav>
      <div className="sidebar-foot"><span className={notice.tone === 'bad' ? 'signal bad' : 'signal'}></span><div><b>{busy ? 'SYNCING' : 'LOCAL CONTROL'}</b><small>KernelSU · xiaod</small></div></div>
    </aside>
    <main className="workspace" aria-busy={busy}>
      <header className="topbar">
        <div><h1>{SECTION_MAP[view]?.[1]}</h1><p className="topbar-context">{SECTION_MAP[view]?.[2]}</p></div>
        <div className="top-actions"><span className="version">v0.2.8</span><button className="icon-button" type="button" onClick={() => refresh()} disabled={busy} aria-label="Refresh current view"><Icon name="refresh" /></button></div>
      </header>
      <div className={`notice ${notice.tone}`} role={notice.tone === 'bad' ? 'alert' : 'status'} aria-live={notice.tone === 'bad' ? 'assertive' : 'polite'}><span></span>{notice.text}</div>
      <section className="view-content">{content}</section>
    </main>
    {confirm && <ConfirmDialog {...confirm} busy={busy} onClose={() => setConfirm(null)} />}
    {prompt && <PromptDialog {...prompt} busy={busy} onClose={() => setPrompt(null)} />}
    {profileEditor && <ProfileEditor profile={profileEditor === 'new' ? null : profileEditor} busy={busy} onClose={() => setProfileEditor(null)} onError={message => setNotice({ tone: 'bad', text: message })} onSave={async body => {
      const result = await action('provider-custom', body, body.action === 'create' ? 'Custom profile created with isolated write-only secret references.' : 'Custom profile committed. Any obsolete secret cleanup warning is recorded separately.', 'providers');
      if (result) setProfileEditor(null);
      return result;
    }} />}
    {sessionEditor && <SessionAiDialog session={sessionEditor} profiles={providers?.custom_profiles || []} busy={busy} onClose={() => setSessionEditor(null)} onApply={useSessionAi} />}
  </div>;
}

function Panel({ title, eyebrow, action, children, className = '' }) {
  return <article className={`panel ${className}`}><div className="panel-head"><div>{eyebrow && <span className="eyebrow">{eyebrow}</span>}<h2>{title}</h2></div>{action}</div>{children}</article>;
}

function Status({ value }) { return <span className={`status ${classForStatus(value)}`}>{titleCase(value)}</span>; }
function Empty({ children }) { return <div className="empty">{children}</div>; }
function Button({ children, tone = 'quiet', type = 'button', ...props }) { return <button type={type} className={`button ${tone}`} {...props}>{children}</button>; }

function Rows({ rows }) {
  return <div className="rows">{rows.map(([label, value, status]) => <div className="row" key={label}><span>{label}</span><b className={status ? classForStatus(status) : ''}>{value ?? '—'}</b></div>)}</div>;
}

function Pager({ value, onChange }) {
  if (!value || Number(value.pages || 1) <= 1) return null;
  return <nav className="pager" aria-label="Pagination"><Button disabled={value.page <= 1} onClick={() => onChange(value.page - 1)} aria-label="Previous page"><Icon name="previous" /></Button><span>PAGE {value.page} / {value.pages}</span><Button disabled={value.page >= value.pages} onClick={() => onChange(value.page + 1)} aria-label="Next page"><Icon name="next" /></Button></nav>;
}

function DashboardView({ data, setSessionEditor, refreshProviders }) {
  if (!data) return <Loading />;
  const health = data.health || {}; const counts = data.counts || {}; const runtime = data.runtime || {}; const ai = data.current_ai || {};
  const metrics = [['Sessions', counts.sessions], ['Runs', counts.agent_runs], ['Attachments', counts.attachments], ['Approvals', counts.pending_approvals], ['Memory', counts.memories], ['Skills', counts.skills], ['Active', counts.running_runs], ['Blocked', counts.blocked_runs]];
  return <>
    <section className="command-surface"><div><span className="eyebrow">INSTALLATION PULSE</span><h2>{counts.running_runs ? `${counts.running_runs} run${counts.running_runs === 1 ? '' : 's'} in motion` : 'Control plane is quiet and ready'}</h2><p>One durable installation owner. SQLite-authoritative Telegram control. Custom endpoint credentials stay write-only.</p></div><div className="runtime-stamps"><span>ROOT {runtime.root ? 'READY' : 'UNAVAILABLE'}</span><span>TERMUX {runtime.termux ? 'READY' : 'ABSENT'}</span><span>DB {health.db_healthy ? 'HEALTHY' : 'FAILED'}</span></div></section>
    <section className="metrics">{metrics.map(([label, value]) => <div className="metric" key={label}><span>{label}</span><strong>{value || 0}</strong><small>durable records</small></div>)}</section>
    <div className="grid two"><Panel title="Active session AI" eyebrow="EXACT SELECTION" action={<Button disabled={!ai.session_id} onClick={async () => { const profiles = await refreshProviders(); if (profiles) setSessionEditor({ id: ai.session_id, name: 'Current active session', provider: ai.provider, account_or_profile_id: ai.account_or_profile_id, model: ai.model }); }}>Change AI</Button>}><Rows rows={[["Provider", ai.provider], ["Custom profile", ai.account_or_profile_id], ["Model", ai.model], ["Session", ai.session_id]]} /></Panel><Panel title="Service health" eyebrow="LIVE"><Rows rows={[["Gateway", health.gateway, health.gateway], ["Database", health.db_healthy ? 'healthy' : 'failed', health.db_healthy ? 'ready' : 'failed'], ["Telegram", health.telegram_polling ? 'polling' : (health.telegram_enabled ? 'waiting' : 'disabled'), health.telegram_polling ? 'ready' : 'warn'], ["Custom ready", health.providers_ready], ["Uptime", formatDuration(health.uptime_seconds)], ["Memory RSS", formatBytes(health.memory_bytes)]]} /></Panel></div>
  </>;
}

function SetupView({ data, action, busy, setNotice }) {
  const telegram = data?.telegram || {};
  const [draft, setDraft] = useState({ enabled: false, owner: '', chats: '', token: '', confirmOwnerChange: false });
  useEffect(() => setDraft({ enabled: Boolean(telegram.enabled), owner: telegram.owner_user_id ?? '', chats: (telegram.allowed_chat_ids || []).join(', '), token: '', confirmOwnerChange: false }), [telegram.enabled, telegram.owner_user_id, telegram.allowed_chat_ids]);
  if (!data) return <Loading />;
  const save = async actionName => {
    const owner = Number(draft.owner);
    if (!Number.isSafeInteger(owner) || owner === 0) {
      setNotice({ tone: 'bad', text: 'Owner Telegram user ID must be a non-zero integer.' });
      return;
    }
    const allowed = draft.chats.split(',').map(value => value.trim()).filter(Boolean).map(Number);
    if (allowed.some(value => !Number.isSafeInteger(value) || value === 0)) {
      setNotice({ tone: 'bad', text: 'Allowed chat IDs must be non-zero integers separated by commas.' });
      return;
    }
    const ownerChanged = telegram.owner_user_id !== undefined && telegram.owner_user_id !== null && Number(telegram.owner_user_id) !== owner;
    if (ownerChanged && !draft.confirmOwnerChange) {
      setNotice({ tone: 'bad', text: 'Confirm the Telegram owner replacement before committing the new binding.' });
      return;
    }
    await action('telegram', { action: actionName, enabled: draft.enabled, owner_user_id: owner, allowed_chat_ids: [...new Set(allowed)], confirm_owner_change: ownerChanged, ...(draft.token.trim() ? { token: draft.token.trim() } : {}) }, actionName === 'save_and_test' ? 'Telegram state committed and tested. Runtime reload follows commit.' : 'Telegram control state committed. Runtime reload follows commit.', 'setup');
    setDraft(current => ({ ...current, token: '' }));
  };
  const ownerChanged = telegram.owner_user_id !== undefined && telegram.owner_user_id !== null && Number(telegram.owner_user_id) !== Number(draft.owner);
  return <div className="grid two"><Panel title="Telegram control plane" eyebrow="SQLITE AUTHORITATIVE"><form className="form-grid" onSubmit={event => { event.preventDefault(); save('save'); }}><Field label="Bot token" hint="Write-only. Leave blank to retain the active immutable secret reference."><input type="password" autoComplete="new-password" value={draft.token} onChange={event => setDraft({ ...draft, token: event.target.value })} placeholder="••••••••••••••••" /></Field><Field label="Owner Telegram user ID"><input type="number" required value={draft.owner} onChange={event => setDraft({ ...draft, owner: event.target.value })} placeholder="123456789" /></Field><Field label="Allowed chat IDs" hint="Optional, comma-separated scope restriction" wide><input value={draft.chats} onChange={event => setDraft({ ...draft, chats: event.target.value })} placeholder="-1001234567890, -1009876543210" /></Field>{ownerChanged && <label className="retention wide"><b>Owner binding replacement</b><p>This changes only the Telegram authentication binding; the durable installation owner and owner data remain unchanged.</p><span className="toggle"><input type="checkbox" checked={draft.confirmOwnerChange} onChange={event => setDraft({ ...draft, confirmOwnerChange: event.target.checked })} /> I confirm this owner replacement</span></label>}<label className="toggle wide"><input type="checkbox" checked={draft.enabled} onChange={event => setDraft({ ...draft, enabled: event.target.checked })} /> Enable Telegram adapter</label><div className="form-actions wide"><Button tone="secondary" disabled={busy} onClick={() => save('save')}>Save control state</Button><Button tone="primary" disabled={busy} onClick={() => save('save_and_test')}>Save & test</Button><Button disabled={busy} onClick={() => action('telegram', { action: 'test', ...(draft.token.trim() ? { token: draft.token.trim() } : {}) }, 'Telegram getMe completed; no configuration was changed.', 'setup')}>Test token</Button></div></form></Panel><Panel title="Current transport state" eyebrow="NO FILE/DB SPLIT"><Rows rows={[["Bot token", telegram.token_configured ? 'configured (write-only)' : 'not configured', telegram.token_configured ? 'ready' : 'warn'], ["Bot identity", telegram.bot?.username ? `@${telegram.bot.username} · ${telegram.bot.id}` : (telegram.bot?.id || 'not tested')], ["Owner state", telegram.owner_state, telegram.owner_state === 'configured' ? 'ready' : 'warn'], ["Allowed chats", (telegram.allowed_chat_ids || []).length || 'all owner chats'], ["Legacy candidates", telegram.legacy_candidate_count || 0]]} /><p className="helper">The database is authoritative. Compatibility TOML is a one-way projection and cannot roll back committed Telegram state.</p></Panel></div>;
}

function AgentView({ data, action, busy }) {
  const settings = data?.settings || {};
  const [draft, setDraft] = useState(settings);
  useEffect(() => setDraft(settings), [data]);
  if (!data) return <Loading />;
  const number = (key, min, max) => <Field label={titleCase(key)} hint={`${min}–${max}`}><input type="number" min={min} max={max} value={draft[key] ?? ''} onChange={event => setDraft({ ...draft, [key]: Number(event.target.value) })} /></Field>;
  const toggle = key => <label className="toggle"><input type="checkbox" checked={Boolean(draft[key])} onChange={event => setDraft({ ...draft, [key]: event.target.checked })} /> {titleCase(key)}</label>;
  return <form onSubmit={event => { event.preventDefault(); action('agent', { action: 'update', ...draft }, 'Agent settings saved atomically. New runs use the updated snapshot.', 'agent'); }}>
    <div className="grid two"><Panel title="Loop limits" eyebrow="BOUNDED"><div className="form-grid">{number('max_turns', 2, 500)}{number('max_tool_calls', 1, 512)}{number('max_runtime_seconds', 10, 3600)}{number('max_no_progress_repeats', 1, 10)}</div></Panel><Panel title="Performance" eyebrow="NEW RUN SNAPSHOT"><div className="form-grid">{number('max_parallel_readonly_tools', 1, 16)}{number('max_execution_plan_steps', 1, 64)}{toggle('provider_streaming')}{toggle('parallel_readonly_tools')}{toggle('execution_plan_enabled')}{toggle('plan_cache_enabled')}{toggle('background_learning')}</div></Panel></div>
    <Panel title="Effective daemon state" eyebrow="DIAGNOSTICS"><Rows rows={[["Config generation", data.generation], ["Loaded", data.loaded ? 'yes' : 'no', data.loaded ? 'ready' : 'warn'], ["Active runs", data.active_runs || 0]]} /><div className="panel-actions"><Button tone="primary" type="submit" disabled={busy}>Save Agent settings</Button></div></Panel>
  </form>;
}

function ProvidersView({ data, action, setConfirm, setProfileEditor }) {
  if (!data) return <Loading />;
  const profiles = data.custom_profiles || [];
  return <>
    <section className="section-intro"><div><span className="eyebrow">CUSTOM ONLY</span><h2>OpenAI-compatible profiles</h2><p>Each profile owns isolated API-key and secret-header references. Values are never returned to this browser.</p></div><Button tone="primary" onClick={() => setProfileEditor('new')}>Add Custom profile</Button></section>
    <div className="profile-grid">{profiles.length ? profiles.map(profile => <ProfileCard key={profile.id} profile={profile} action={action} setConfirm={setConfirm} setProfileEditor={setProfileEditor} />) : <Empty>No Custom profile exists. Create one to discover and exact-probe its models.</Empty>}</div>
  </>;
}

function ProfileCard({ profile, action, setConfirm, setProfileEditor }) {
  const models = profile.models || [];
  return <Panel title={profile.alias} eyebrow="CUSTOM PROFILE" action={<Status value={profile.reachability} />} className="profile-card"><p className="endpoint">{profile.endpoint}</p><div className="chip-row"><span>{titleCase(profile.protocol)}</span><span>{profile.api_key_configured ? 'API key configured' : 'No API key'}</span><span>{(profile.header_names || []).length} header name{(profile.header_names || []).length === 1 ? '' : 's'}</span></div><div className="model-list">{models.length ? models.slice(0, 6).map(model => <div className="model-row" key={model.model_id}><div><b>{model.model_id}</b><small>{modelReadiness(model)}</small></div><div><span>tools {titleCase(model.tool_protocol)}</span><span>vision {titleCase(model.vision_state)}</span><span>file {titleCase(model.file_input_state)}</span></div></div>) : <Empty>No discovered models yet. Test this endpoint first.</Empty>}</div><div className="card-actions"><Button tone="secondary" onClick={() => action('provider-custom', { action: 'test', profile_id: profile.id }, `Model discovery and bounded capability probes finished for ${profile.alias}.`, 'providers')}>Discover & test</Button><Button onClick={() => setProfileEditor(profile)}>Edit</Button><Button tone="danger" onClick={() => setConfirm({ title: `Delete ${profile.alias}?`, body: 'This removes the profile row. Old secrets are cleaned after the committed delete and any cleanup warning is recorded without faking a rollback.', confirmLabel: 'Delete profile', onConfirm: () => action('provider-custom', { action: 'delete', profile_id: profile.id }, `Custom profile ${profile.alias} deleted.`, 'providers') })}>Delete</Button></div></Panel>;
}

function SessionsView({ data, action, setConfirm, setPrompt, setSessionEditor, refreshProviders, changePage }) {
  if (!data) return <Loading />;
  const items = data.items || [];
  return <><section className="section-intro"><div><span className="eyebrow">CONVERSATION CONTROL</span><h2>Main sessions</h2><p>Deleting a session permanently removes its conversation-owned records and unreferenced attachments. Owner-global memory, skills, and Custom profiles remain intact.</p></div><Button tone="primary" onClick={() => action('sessions', { action: 'new' }, 'New main session created with YOLO off.', 'sessions')}>New session</Button></section><Panel title="Sessions" eyebrow="5 / PAGE"><div className="session-table"><div className="session-head"><span>Session</span><span>AI</span><span>Messages</span><span>Controls</span></div>{items.length ? items.map(session => <div className="session-line" key={session.id}><div><b>{session.name}</b><small>{session.telegram_scope ? `Chat ${session.telegram_scope.chat_id} · topic ${session.telegram_scope.message_thread_id ?? 'default'}` : 'Local main session'}</small></div><div><b>{session.model}</b><small>{session.provider === 'custom' ? 'Custom' : 'Legacy profile; select Custom before generation'}</small></div><div><b>{session.message_count}</b><small>YOLO {session.yolo ? 'ON' : 'OFF'}</small></div><div className="table-actions"><Button onClick={async () => { await refreshProviders(); setSessionEditor(session); }}>AI</Button><Button onClick={() => action('sessions', { action: 'yolo', session_id: session.id, value: String(!session.yolo) }, `YOLO ${session.yolo ? 'disabled' : 'enabled'} for ${session.name} only.`, 'sessions')}>{session.yolo ? 'YOLO off' : 'YOLO on'}</Button><Button onClick={() => setPrompt({ title: 'Rename session', label: 'Session name', initial: session.name, confirmLabel: 'Rename', onConfirm: value => action('sessions', { action: 'rename', session_id: session.id, value }, 'Session renamed.', 'sessions') })}>Rename</Button><Button tone="danger" onClick={() => setConfirm({ title: `Delete ${session.name}?`, body: 'This is permanent. If the deleted session is active, Xiao atomically creates or activates a valid replacement main session.', confirmLabel: 'Delete session', onConfirm: () => action('sessions', { action: 'delete', session_id: session.id }, 'Session deleted; a valid main-session pointer was recovered.', 'sessions') })}>Delete</Button></div></div>) : <Empty>No main session exists yet.</Empty>}</div><Pager value={data} onChange={next => changePage('sessions', next)} /></Panel></>;
}

function AttachmentsView({ data, action, setConfirm, changePage }) {
  if (!data) return <Loading />;
  const usage = data.usage || {}; const items = data.items || [];
  return <><section className="usage-grid"><div><span>Owner usage</span><strong>{formatBytes(usage.owner_bytes ?? usage.owner_usage_bytes)}</strong></div><div><span>Global usage</span><strong>{formatBytes(usage.global_bytes ?? usage.global_usage_bytes)}</strong></div><div><span>Attachments</span><strong>{usage.count ?? items.length}</strong></div><div><span>Reservation cleanup</span><strong>startup-safe</strong></div></section><Panel title="Attachment processing" eyebrow="QUOTA / OCR / PROVIDER FALLBACK"><div className="attachment-list">{items.length ? items.map(item => <div className="attachment-row" key={item.attachment_id}><div><b>{item.original_name}</b><small>{item.detected_mime} · {formatBytes(item.size_bytes)} · session {item.session_id}</small></div><div><Status value={item.processing_status} /><small>{item.error || item.summary || 'No processing detail'}</small></div><Button tone="danger" onClick={() => setConfirm({ title: `Remove ${item.original_name}?`, body: 'The durable attachment record and its managed file/chunks will be removed.', confirmLabel: 'Remove attachment', onConfirm: () => action('attachments', { action: 'remove', attachment_id: item.attachment_id }, 'Attachment removed through AttachmentManager lifecycle.', 'attachments') })}>Remove</Button></div>) : <Empty>No attachments have been recorded.</Empty>}</div><Pager value={data} onChange={next => changePage('attachments', next)} /></Panel></>;
}

function TasksView({ data, action, setConfirm, changePage }) {
  if (!data) return <Loading />;
  const active = new Set(['received', 'context_build', 'running', 'awaiting_approval', 'verifying']);
  return <Panel title="Agent runs" eyebrow="OBSERVABLE EXECUTION"><div className="run-list">{(data.items || []).length ? data.items.map(run => <div className="run-card" key={run.id}><div className="run-title"><div><b>{run.goal || 'Untitled run'}</b><small>{run.provider} / {run.model} · {formatDate(run.started_at)}</small></div><Status value={run.status} /></div><p>{run.blocker_or_error || run.result || 'No final result recorded.'}</p><div className="chip-row"><span>session {run.session_id}</span><span>verification {run.verification?.state || 'unknown'}</span>{(run.tools || []).slice(0, 3).map(tool => <span key={tool.id || tool.tool_name}>{tool.tool_name}:{tool.status}</span>)}</div>{active.has(run.status) && <div className="card-actions"><Button tone="danger" onClick={() => setConfirm({ title: 'Stop this run?', body: 'Cancellation is propagated to provider, tool, Termux, attachment, OCR, and fallback work where the runtime can enforce it.', confirmLabel: 'Stop run', onConfirm: () => action('runs', { action: 'cancel', run_id: run.id }, 'Cancellation requested for the active run.', 'tasks') })}>Stop run</Button></div>}</div>) : <Empty>No agent run has been recorded.</Empty>}</div><Pager value={data} onChange={next => changePage('tasks', next)} /></Panel>;
}

function MemoryView({ data, action, filters, setFilters, changePage }) {
  const [draft, setDraft] = useState({ scope: 'user', category: '', key: '', value: '' });
  if (!data) return <Loading />;
  return <><section className="section-intro"><div><span className="eyebrow">CANONICAL OWNER STATE</span><h2>Memory manager</h2><p>WebUI remains the full manager. Telegram only exposes the normal chat surface.</p></div><Button onClick={() => action('memory', { action: 'reconcile' }, 'Canonical USER.md and MEMORY.md were reconciled.', 'memory')}>Reconcile files</Button></section><div className="filter"><input value={filters.memory} onChange={event => setFilters({ ...filters, memory: event.target.value })} placeholder="Search active memory" /><Button tone="secondary" onClick={() => { changePage('memory', 1); }}>Search</Button></div><div className="grid two"><Panel title="Current memory" eyebrow="5 / PAGE"><div className="card-list">{(data.items || []).length ? data.items.map(item => <div className="compact-card" key={`${item.scope}:${item.category}:${item.key}`}><div><b>{item.key}</b><small>{item.scope} / {item.category} · {item.source_kind}</small></div><p>{item.value}</p><div className="card-actions"><Button onClick={() => setDraft({ scope: item.scope, category: item.category, key: item.key, value: item.value })}>Edit</Button><Button tone="danger" onClick={() => action('memory', { action: 'delete', scope: item.scope, category: item.category, key: item.key }, 'Memory entry removed; audit history remains.', 'memory')}>Forget</Button></div></div>) : <Empty>No active memory matches this query.</Empty>}</div><Pager value={data} onChange={next => changePage('memory', next)} /></Panel><Panel title="Create or replace" eyebrow="MANAGED WRITE"><form className="form-grid" onSubmit={event => { event.preventDefault(); action('memory', { action: 'upsert', ...draft }, 'Current memory state saved.', 'memory'); }}><Field label="Scope"><select value={draft.scope} onChange={event => setDraft({ ...draft, scope: event.target.value })}><option value="user">User</option><option value="agent">Agent</option></select></Field><Field label="Category"><input required value={draft.category} onChange={event => setDraft({ ...draft, category: event.target.value })} /></Field><Field label="Key" wide><input required value={draft.key} onChange={event => setDraft({ ...draft, key: event.target.value })} /></Field><Field label="Current value" wide><textarea required value={draft.value} onChange={event => setDraft({ ...draft, value: event.target.value })} /></Field><div className="form-actions wide"><Button tone="primary" type="submit">Save memory</Button></div></form></Panel></div></>;
}

function SkillsView({ data, action, filters, setFilters, changePage, setConfirm }) {
  if (!data) return <Loading />;
  return <><div className="filter"><input value={filters.skills} onChange={event => setFilters({ ...filters, skills: event.target.value })} placeholder="Search learned or imported skills" /><Button tone="secondary" onClick={() => changePage('skills', 1)}>Search</Button><Button onClick={() => action('skills', { action: 'refresh' }, 'Filesystem skill index refreshed.', 'skills')}>Rescan</Button></div><Panel title="Skill library" eyebrow="FULL WEBUI MANAGEMENT"><div className="card-list">{(data.items || []).length ? data.items.map(skill => <div className="compact-card" key={skill.id}><div><b>{skill.name}</b><small>{skill.source_kind} · {skill.enabled ? 'enabled' : 'disabled'}</small></div><p>{skill.summary}</p><div className="chip-row"><span>when: {skill.when_to_use}</span><span>verify: {skill.verification}</span></div><div className="card-actions"><Button onClick={() => action('skills', { action: 'set_enabled', skill_id: skill.id, enabled: !skill.enabled }, `Skill ${skill.enabled ? 'disabled' : 'enabled'}.`, 'skills')}>{skill.enabled ? 'Disable' : 'Enable'}</Button>{skill.source_kind === 'learned' && <Button tone="danger" onClick={() => setConfirm({ title: `Delete learned skill ${skill.name}?`, body: 'Imported/community skills are not removed here. This only removes the learned durable skill.', confirmLabel: 'Delete skill', onConfirm: () => action('skills', { action: 'delete', skill_id: skill.id }, 'Learned skill deleted.', 'skills') })}>Delete</Button>}</div></div>) : <Empty>No skill matches this query.</Empty>}</div><Pager value={data} onChange={next => changePage('skills', next)} /></Panel></>;
}

function ToolsView({ data }) { if (!data) return <Loading />; return <Panel title="Typed capability registry" eyebrow="POLICY ENFORCED"><CapabilityTable items={data.items || []} /></Panel>; }
function RuntimeView({ data }) { if (!data) return <Loading />; const environment = data.environment || {}; return <div className="grid two"><Panel title="Device environment" eyebrow="PROBED"><Rows rows={Object.entries(environment).filter(([key]) => !['binaries', 'termux'].includes(key)).map(([key, value]) => [titleCase(key), typeof value === 'object' ? JSON.stringify(value) : String(value)])} /></Panel><Panel title="Termux / execution truth" eyebrow="UID / GID / PATH"><Rows rows={Object.entries(environment.termux || {}).map(([key, value]) => [titleCase(key), String(value)])} /></Panel><Panel title="Registered capabilities" eyebrow="RUNTIME"><CapabilityTable items={data.capabilities || []} /></Panel><Panel title="Managed paths" eyebrow="PRIVATE"><Rows rows={Object.entries(data.paths || {}).map(([key, value]) => [titleCase(key), value])} /></Panel></div>; }

function SecurityView({ data, action }) {
  if (!data) return <Loading />;
  return <><div className="security-note">{data.admin_bind_loopback ? 'Admin API is loopback-only.' : 'FAIL: Admin API is not loopback-only.'} Raw unrestricted root shell: {data.root_shell_exposed ? 'EXPOSED' : 'disabled'}.</div><div className="grid two"><Panel title="Pending approvals" eyebrow="EXACT / ONE-SHOT"><div className="card-list">{(data.pending_approvals || []).length ? data.pending_approvals.map(approval => <div className="compact-card" key={approval.id}><div><b>{approval.tool_name}</b><Status value={approval.status} /></div><p>{approval.summary}</p><small>session {approval.session_id} · expires {formatDate(approval.expires_at)}</small><div className="card-actions"><Button tone="primary" onClick={() => action('security', { action: 'approve', approval_id: approval.id }, 'Exact operation approved once.', 'security')}>Approve once</Button><Button tone="danger" onClick={() => action('security', { action: 'deny', approval_id: approval.id }, 'Exact operation denied.', 'security')}>Deny</Button></div></div>) : <Empty>No pending approval.</Empty>}</div></Panel><Panel title="YOLO sessions" eyebrow="ASK ONLY"><div className="card-list">{(data.yolo_sessions || []).length ? data.yolo_sessions.map(session => <div className="compact-card" key={session.id}><b>{session.name}</b><small>{session.provider} / {session.model}</small></div>) : <Empty>YOLO is off in every active main session.</Empty>}</div></Panel></div><Panel title="Recent security audit" eyebrow="REDACTED"><div className="audit-list">{(data.recent_audit || []).length ? data.recent_audit.map(item => <div key={`${item.created_at}:${item.action}`}><time>{formatDate(item.created_at)}</time><b>{item.action}</b><code>{item.detail}</code></div>) : <Empty>No audit event recorded.</Empty>}</div></Panel></>;
}

function DiagnosticsView({ data, refresh, busy }) { if (!data) return <Loading />; return <Panel title="Independent diagnostics" eyebrow="READ ONLY" action={<Button tone="primary" disabled={busy} onClick={() => refresh('diagnostics')}>Run diagnostics</Button>}><div className="diagnostics">{(data.checks || []).map(item => <div className="diagnostic" key={item.name}><Status value={item.status} /><div><b>{item.name}</b><p>{item.source} · {item.evidence}</p></div></div>)}</div></Panel>; }

function LogsView({ data, filters, setFilters, refresh }) { if (!data) return <Loading />; return <Panel title="Redacted daemon logs" eyebrow="LOCAL ONLY" action={<div className="inline-actions"><select value={filters.logLines} onChange={event => setFilters({ ...filters, logLines: Number(event.target.value) })}><option value={100}>100 lines</option><option value={250}>250 lines</option><option value={500}>500 lines</option></select><Button onClick={() => refresh('logs')}>Refresh</Button></div>}><pre>{(data.lines || []).join('\n') || 'No daemon log entries.'}</pre></Panel>; }

function CapabilityTable({ items }) { return <div className="table-scroll"><table><thead><tr><th>Capability</th><th>State</th><th>Backend</th><th>Evidence</th></tr></thead><tbody>{items.map(item => <tr key={item.name}><td>{item.name}</td><td><Status value={item.status} /></td><td>{item.backend || '—'}</td><td>{item.evidence || '—'}</td></tr>)}</tbody></table></div>; }

function Field({ label, hint, wide = false, children }) { return <label className={wide ? 'field wide' : 'field'}><span>{label}</span>{hint && <small>{hint}</small>}{children}</label>; }
function Loading() { return <div className="loading" role="status" aria-live="polite"><span></span>Reading durable state from xiaod…</div>; }

function Dialog({ labelId, onClose, className = '', children }) {
  const dialog = useRef(null);
  const previousFocus = useRef(null);
  const onCloseRef = useRef(onClose);
  useEffect(() => { onCloseRef.current = onClose; }, [onClose]);
  useEffect(() => {
    previousFocus.current = document.activeElement;
    const focus = () => {
      const initial = dialog.current?.querySelector('[data-dialog-initial-focus], input, select, textarea, button:not([disabled])');
      (initial || dialog.current)?.focus();
    };
    const timer = window.setTimeout(focus, 0);
    const handleKey = event => {
      if (event.key === 'Escape') {
        event.preventDefault();
        onCloseRef.current();
        return;
      }
      if (event.key !== 'Tab') return;
      const focusable = [...(dialog.current?.querySelectorAll('a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])') || [])]
        .filter(element => !element.hidden && element.getClientRects().length);
      if (!focusable.length) {
        event.preventDefault();
        dialog.current?.focus();
        return;
      }
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };
    window.addEventListener('keydown', handleKey);
    return () => {
      window.clearTimeout(timer);
      window.removeEventListener('keydown', handleKey);
      if (previousFocus.current instanceof HTMLElement && previousFocus.current.isConnected) previousFocus.current.focus();
    };
  }, []);
  return <div className="modal-backdrop" role="presentation"><section ref={dialog} className={`modal ${className}`} role="dialog" aria-modal="true" aria-labelledby={labelId} tabIndex="-1">{children}</section></div>;
}

function ConfirmDialog({ title, body, confirmLabel, onConfirm, onClose, busy }) {
  const submit = async () => { const result = await onConfirm(); if (result !== null) onClose(); };
  return <Dialog labelId="confirm-title" onClose={onClose}><h2 id="confirm-title">{title}</h2><p>{body}</p><div className="modal-actions"><Button disabled={busy} onClick={onClose}>Cancel</Button><Button tone="danger" disabled={busy} onClick={submit}>{confirmLabel || 'Confirm'}</Button></div></Dialog>;
}

function PromptDialog({ title, label, initial = '', confirmLabel, onConfirm, onClose, busy }) {
  const [value, setValue] = useState(initial);
  const submit = async event => { event.preventDefault(); if (!value.trim()) return; const result = await onConfirm(value.trim()); if (result !== null) onClose(); };
  return <Dialog labelId="prompt-title" onClose={onClose}><form onSubmit={submit}><h2 id="prompt-title">{title}</h2><Field label={label}><input data-dialog-initial-focus value={value} maxLength={120} onChange={event => setValue(event.target.value)} /></Field><div className="modal-actions"><Button disabled={busy} onClick={onClose}>Cancel</Button><Button tone="primary" type="submit" disabled={busy}>{confirmLabel || 'Save'}</Button></div></form></Dialog>;
}

function ProfileEditor({ profile, busy, onClose, onSave, onError }) {
  const [draft, setDraft] = useState(emptyProfile(profile));
  useEffect(() => setDraft(emptyProfile(profile)), [profile]);
  const endpointChanged = Boolean(profile && draft.endpoint.trim() !== profile.endpoint);
  const configuredHeaderCount = (profile?.header_names || []).length;
  const configuredKey = Boolean(profile?.api_key_configured);
  const submit = async event => {
    event.preventDefault();
    let headers; let secretHeaders;
    try { headers = parseHeaderObject(draft.safeHeaders, 'Safe'); secretHeaders = parseHeaderObject(draft.secretHeaders, 'Secret'); }
    catch (error) { onError(error.message); return; }
    if (draft.keyAction === 'replace' && !draft.apiKey.trim()) {
      onError('Enter a replacement API key or choose a different key action.');
      return;
    }
    const body = {
      action: profile ? 'edit' : 'create',
      ...(profile ? { profile_id: profile.id } : {}),
      alias: draft.alias.trim(), endpoint: draft.endpoint.trim(), protocol: draft.protocol,
      ...(headers !== undefined ? { headers } : {}),
      ...(secretHeaders !== undefined ? { secret_headers: secretHeaders } : {}),
      ...(draft.keyAction === 'replace' ? { api_key: draft.apiKey.trim() } : {}),
      remove_api_key: draft.keyAction === 'remove',
      clear_secret_headers: draft.clearSecretHeaders,
      keep_credential: endpointChanged && draft.keepCredential,
      keep_safe_headers: endpointChanged && draft.keepSafeHeaders,
      keep_secret_headers: endpointChanged && draft.keepSecretHeaders
    };
    await onSave(body);
  };
  const safeHeaderHint = endpointChanged
    ? 'Blank clears old safe headers. Supply JSON here to bind replacements to the new endpoint.'
    : 'Blank preserves the current safe headers. Use {} to clear them.';
  const secretHeaderHint = endpointChanged
    ? 'Blank clears old secret headers. Supply replacement JSON for the new endpoint; values stay write-only.'
    : 'Blank preserves current secret headers. Values are write-only; use Clear stored secret headers to remove them.';
  return <Dialog labelId="profile-editor-title" onClose={onClose} className="modal-large"><form onSubmit={submit}><div className="modal-top"><div><h2 id="profile-editor-title">{profile ? `Edit ${profile.alias}` : 'New Custom profile'}</h2><p className="modal-context">Custom-only endpoint credentials</p></div><Button onClick={onClose}>Close</Button></div><p>Secret values are submitted only to xiaod and never rendered back into this page.</p>{profile && <div className="profile-presence" aria-label="Current profile credential state"><span>{configuredKey ? 'API key configured' : 'No API key configured'}</span><span>{configuredHeaderCount} configured header name{configuredHeaderCount === 1 ? '' : 's'}</span></div>}<div className="form-grid"><Field label="Alias"><input data-dialog-initial-focus required maxLength={80} value={draft.alias} onChange={event => setDraft({ ...draft, alias: event.target.value })} placeholder="studio-local" /></Field><Field label="Endpoint"><input required type="url" value={draft.endpoint} onChange={event => setDraft({ ...draft, endpoint: event.target.value })} placeholder="https://api.example.com/v1" /></Field><Field label="Protocol"><select value={draft.protocol} onChange={event => setDraft({ ...draft, protocol: event.target.value })}><option value="openai_chat_completions">OpenAI Chat Completions</option><option value="openai_responses">OpenAI Responses</option></select></Field><Field label="API key"><select value={draft.keyAction} onChange={event => setDraft({ ...draft, keyAction: event.target.value })}><option value="keep">{profile ? 'Keep current key' : 'No API key'}</option><option value="replace">Set replacement key</option>{profile && <option value="remove">Remove key</option>}</select></Field>{draft.keyAction === 'replace' && <Field label="Replacement API key" wide><input type="password" autoComplete="new-password" value={draft.apiKey} onChange={event => setDraft({ ...draft, apiKey: event.target.value })} /></Field>}<Field label="Safe headers" hint={safeHeaderHint} wide><textarea rows="4" value={draft.safeHeaders} onChange={event => setDraft({ ...draft, safeHeaders: event.target.value })} placeholder='{"X-Workspace":"personal"}' /></Field><Field label="Secret headers" hint={secretHeaderHint} wide><textarea rows="4" value={draft.secretHeaders} onChange={event => setDraft({ ...draft, secretHeaders: event.target.value })} placeholder='{"Authorization":"Bearer …"}' /></Field>{profile && !endpointChanged && <label className="toggle wide"><input type="checkbox" checked={draft.clearSecretHeaders} onChange={event => setDraft({ ...draft, clearSecretHeaders: event.target.checked })} /> Clear stored secret headers</label>}{endpointChanged && <div className="retention wide"><b>Endpoint trust boundary changed</b><p>Model discovery and probes are invalidated. The default clears old credentials and headers; replacements entered above are committed in this same patch.</p>{configuredKey && <label><input type="checkbox" checked={draft.keepCredential} onChange={event => setDraft({ ...draft, keepCredential: event.target.checked })} /> Explicitly retain the existing API key</label>}{configuredHeaderCount > 0 && <><label><input type="checkbox" checked={draft.keepSafeHeaders} onChange={event => setDraft({ ...draft, keepSafeHeaders: event.target.checked })} /> Explicitly retain existing safe headers</label><label><input type="checkbox" checked={draft.keepSecretHeaders} onChange={event => setDraft({ ...draft, keepSecretHeaders: event.target.checked })} /> Explicitly retain existing secret headers</label></>}</div>}<div className="modal-actions wide"><Button disabled={busy} onClick={onClose}>Cancel</Button><Button tone="primary" type="submit" disabled={busy}>{profile ? 'Commit profile update' : 'Create isolated profile'}</Button></div></div></form></Dialog>;
}

function emptyProfile(profile) { return { alias: profile?.alias || '', endpoint: profile?.endpoint || '', protocol: profile?.protocol || 'openai_chat_completions', keyAction: 'keep', apiKey: '', safeHeaders: '', secretHeaders: '', clearSecretHeaders: false, keepCredential: false, keepSafeHeaders: false, keepSecretHeaders: false }; }

function SessionAiDialog({ session, profiles, busy, onClose, onApply }) {
  const [profileId, setProfileId] = useState(session.account_or_profile_id || profiles[0]?.id || '');
  const selectedProfile = profiles.find(profile => profile.id === profileId) || profiles[0];
  const [model, setModel] = useState(session.model || selectedProfile?.models?.[0]?.model_id || '');
  useEffect(() => { setProfileId(session.account_or_profile_id || profiles[0]?.id || ''); }, [profiles, session]);
  useEffect(() => { const profile = profiles.find(item => item.id === profileId); if (profile && !profile.models?.some(item => item.model_id === model)) setModel(profile.models?.[0]?.model_id || ''); }, [model, profileId, profiles]);
  const selected = selectedProfile?.models?.find(item => item.model_id === model);
  return <Dialog labelId="session-ai-title" onClose={onClose}><form onSubmit={event => { event.preventDefault(); if (profileId && model) onApply({ sessionId: session.id, profileId, model }); }}><div className="modal-top"><div><h2 id="session-ai-title">Change AI selection</h2><p className="modal-context">Custom-only session selection</p></div><Button onClick={onClose}>Close</Button></div><p>Changes apply only to <b>{session.name}</b>. Legacy Codex/Antigravity history remains readable but cannot be selected for generation.</p>{profiles.length ? <><Field label="Custom profile"><select data-dialog-initial-focus value={profileId} onChange={event => setProfileId(event.target.value)}>{profiles.map(profile => <option key={profile.id} value={profile.id}>{profile.alias} · {profile.endpoint}</option>)}</select></Field><Field label="Exact model"><select value={model} onChange={event => setModel(event.target.value)}>{(selectedProfile?.models || []).map(item => <option key={item.model_id} value={item.model_id}>{item.model_id} — {modelReadiness(item)}</option>)}</select></Field><div className="readiness"><Status value={selected?.probe_status || 'unprobed'} /><span>{requiresExactProbe(selected) ? 'The selected model will be exact-probed before activation.' : `${modelReadiness(selected)}; optional vision/file Unknown does not block text-agent readiness.`}</span></div><div className="modal-actions"><Button onClick={onClose}>Cancel</Button><Button tone="primary" type="submit" disabled={busy || !model}>Apply Custom model</Button></div></> : <Empty>Create and discover a Custom profile before selecting session AI.</Empty>}</form></Dialog>;
}

function formatDuration(value) { let seconds = Number(value || 0); const hours = Math.floor(seconds / 3600); seconds %= 3600; const minutes = Math.floor(seconds / 60); return [hours && `${hours}h`, minutes && `${minutes}m`, `${seconds % 60}s`].filter(Boolean).join(' ') || '0s'; }

export default App;
