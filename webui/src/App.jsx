import React, { useCallback, useEffect, useRef, useState } from 'react';
import { managerGet, managerPost } from './bridge.js';

/* ========== Section Registry ========== */
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
  ['logs', 'Logs', 'Redacted daemon trace'],
];

const VIEW_RESOURCE = { setup: 'telegram' };
function resourceForView(view) { return VIEW_RESOURCE[view] || view; }

const RELOAD_MAP = {
  models: ['providers'],
  profiles: ['providers'],
  'profile-edit': ['providers'],
  telegram: ['setup'],
  agent: ['agent'],
  sessions: ['sessions'],
  'session-detail': ['sessions'],
  attachments: ['attachments'],
  runs: ['tasks'],
  tasks: ['tasks'],
  memory: ['memory'],
  skills: ['skills'],
  tools: ['tools'],
  security: ['security'],
  runtime: ['runtime'],
  diagnostics: ['diagnostics'],
  logs: ['logs'],
  dashboard: ['dashboard'],
};

/* ========== Utilities ========== */
function formatDuration(value) {
  let seconds = Number(value || 0);
  const hours = Math.floor(seconds / 3600); seconds %= 3600;
  const minutes = Math.floor(seconds / 60);
  return [hours && `${hours}h`, minutes && `${minutes}m`, `${seconds % 60}s`].filter(Boolean).join(' ') || '0s';
}

function classForStatus(value) {
  const s = String(value || 'unknown').toLowerCase().replace(/\s/g, '_');
  if (['ready','reachable','completed','available','enabled','pass','supported','configured','running','verified_success'].includes(s)) return 'ok';
  if (['failed','unreachable','denied','error','cancelled','blocked'].includes(s)) return 'bad';
  if (['warn','unknown','unprobed','indeterminate','awaiting_approval','approval_required','missing_installable'].includes(s)) return 'warn';
  return 'muted';
}

function modelReadiness(model) {
  const probe = String(model?.probe_status || 'unprobed').toLowerCase();
  if (probe === 'unprobed') return 'Unprobed';
  if (probe === 'indeterminate') return 'Indeterminate';
  if (model?.native_tools_state === 'supported' && model?.continuation_state === 'supported') return 'Native Agent';
  if (model?.native_tools_state === 'unsupported' && model?.structured_output_state === 'supported' && model?.continuation_state === 'supported') return 'Structured Agent';
  if (model?.native_tools_state === 'unsupported' && model?.structured_output_state === 'unsupported' && model?.continuation_state === 'unsupported') return 'Chat only';
  return 'Protocol indeterminate';
}

function parseHeaderObject(value, kind) {
  const text = String(value || '').trim();
  if (!text) return undefined;
  let parsed;
  try {
    parsed = JSON.parse(text);
  } catch {
    throw new Error(`${kind} headers must be a JSON object.`);
  }
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

/* ========== Base UI Components ========== */
function Button({ tone, onClick, disabled, type = 'button', full, children }) {
  const cls = `btn ${tone === 'primary' ? 'btn-primary' : tone === 'danger' ? 'btn-danger' : tone === 'ghost' ? 'btn-ghost' : 'btn-outline'}${full ? ' btn-full' : ''}`;
  return <button className={cls} type={type} disabled={disabled} onClick={onClick}>{children}</button>;
}

function Empty({ children }) {
  return <div className="empty"><div className="empty-icon">📭</div><p>{children}</p></div>;
}

function Status({ value }) {
  const s = String(value || 'unknown').replace(/_/g, ' ');
  return <span className={`status-badge ${classForStatus(value)}`}>{s}</span>;
}

function Field({ label, hint, wide, children }) {
  return <div className={`form-group${wide ? ' form-wide' : ''}`}>
    <label className="form-label">{label}</label>
    {children}
    {hint && <span className="form-hint">{hint}</span>}
  </div>;
}

function DL({ label, mono, children }) {
  return <div className="detail-row">
    <div className="detail-label">{label}</div>
    <div className={`detail-value${mono ? ' mono' : ''}`}>{children}</div>
  </div>;
}

function Pagination({ page, pages, onPage }) {
  return <div className="pagination">
    <button className="btn btn-outline btn-sm" disabled={page <= 1} onClick={() => onPage(page - 1)}>‹ Prev</button>
    <span>{page} / {pages}</span>
    <button className="btn btn-outline btn-sm" disabled={page >= pages} onClick={() => onPage(page + 1)}>Next ›</button>
  </div>;
}

function Dialog({ labelId, onClose, children, className = '' }) {
  useEffect(() => {
    const handleKey = event => {
      if (event.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', handleKey);
    return () => window.removeEventListener('keydown', handleKey);
  }, [onClose]);
  return <div className="modal-backdrop" role="presentation" onClick={event => { if (event.target === event.currentTarget) onClose(); }}>
    <div className={`modal ${className}`.trim()} role="dialog" aria-modal="true" aria-labelledby={labelId}>
      <div className="modal-handle" />
      {children}
    </div>
  </div>;
}

/* ========== Icons ========== */
const ICONS = {
  spark: 'M12 2L14.5 9.5L22 12L14.5 14.5L12 22L9.5 14.5L2 12L9.5 9.5L12 2Z',
  search: 'M15.5 14h-.79l-.28-.27A6.471 6.471 0 0 0 16 9.5 6.5 6.5 0 1 0 9.5 16c1.61 0 3.09-.59 4.23-1.57l.27.28v.79l5 4.99L20.49 19l-4.99-5zm-6 0C7.01 14 5 11.99 5 9.5S7.01 5 9.5 5 14 7.01 14 9.5 11.99 14 9.5 14z',
  settings: 'M19.14 12.94c.04-.3.06-.61.06-.94 0-.32-.02-.64-.07-.94l2.03-1.58a.49.49 0 0 0 .12-.61l-1.92-3.32a.488.488 0 0 0-.59-.22l-2.39.96c-.5-.38-1.03-.7-1.62-.94l-.36-2.54a.484.484 0 0 0-.48-.41h-3.84c-.24 0-.43.17-.47.41l-.36 2.54c-.59.24-1.13.57-1.62.94l-2.39-.96c-.22-.08-.47 0-.59.22L2.74 8.87c-.12.21-.08.47.12.61l2.03 1.58c-.05.3-.09.63-.09.94s.02.64.07.94l-2.03 1.58a.49.49 0 0 0-.12.61l1.92 3.32c.12.22.37.29.59.22l2.39-.96c.5.38 1.03.7 1.62.94l.36 2.54c.05.24.24.41.48.41h3.84c.24 0 .44-.17.47-.41l.36-2.54c.59-.24 1.13-.56 1.62-.94l2.39.96c.22.08.47 0 .59-.22l1.92-3.32c.12-.22.07-.47-.12-.61l-2.01-1.58zM12 15.6c-1.98 0-3.6-1.62-3.6-3.6s1.62-3.6 3.6-3.6 3.6 1.62 3.6 3.6-1.62 3.6-3.6 3.6z',
  chat: 'M20 2H4c-1.1 0-1.99.9-1.99 2L2 22l4-4h14c1.1 0 2-.9 2-2V4c0-1.1-.9-2-2-2zM6 9h12v2H6V9zm8 5H6v-2h8v2zm4-6H6V6h12v2z',
  plug: 'M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm-1 17.93c-3.95-.49-7-3.85-7-7.93 0-.62.08-1.21.21-1.79L9 15v1c0 1.1.9 2 2 2v1.93zm6.9-2.54c-.26-.81-1-1.39-1.9-1.39h-1v-3c0-.55-.45-1-1-1H8v-2h2c.55 0 1-.45 1-1V7h2c1.1 0 2-.9 2-2v-.41c2.93 1.19 5 4.06 5 7.41 0 2.08-.8 3.97-2.1 5.39z',
  brain: 'M12 3c-4.97 0-9 4.03-9 9 0 2.12.74 4.07 1.97 5.61L4.35 19.4c-.39.39-.39 1.02 0 1.41.39.39 1.02.39 1.41 0l1.9-1.9C9.22 19.59 10.56 20 12 20c4.97 0 9-4.03 9-9s-4.03-9-9-9zm0 15c-3.31 0-6-2.69-6-6s2.69-6 6-6 6 2.69 6 6-2.69 6-6 6z',
  bolt: 'M11 21h-1l1-7H7.5c-.58 0-.57-.32-.38-.66.19-.34.05-.08.07-.12C8.48 10.94 10.42 7.54 13 3h1l-1 7h3.5c.49 0 .56.33.47.51l-.07.15C12.96 17.58 11 21 11 21z',
  tools: 'M22.7 19l-9.1-9.1c.9-2.3.4-5-1.5-6.9-2-2-5-2.4-7.4-1.3L9 6 6 9 1.6 4.7C.4 7.1.9 10.1 2.9 12.1c1.9 1.9 4.6 2.4 6.9 1.5l9.1 9.1c.4.4 1 .4 1.4 0l2.3-2.3c.5-.4.5-1.1.1-1.4z',
  play: 'M8 5v14l11-7z',
  info: 'M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm1 15h-2v-6h2v6zm0-8h-2V7h2v2z',
  telegram: 'M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm4.64 6.8c-.15 1.58-.8 5.42-1.13 7.19-.14.75-.42 1-.68 1.03-.58.05-1.02-.38-1.58-.75-.88-.58-1.38-.94-2.23-1.5-.99-.65-.35-1.01.22-1.59.15-.15 2.71-2.48 2.76-2.69a.2.2 0 0 0-.05-.18c-.06-.05-.14-.03-.21-.02-.09.02-1.49.95-4.22 2.79-.4.27-.76.41-1.08.4-.36-.01-1.04-.2-1.55-.37-.63-.2-1.12-.31-1.08-.66.02-.18.27-.36.74-.55 2.92-1.27 4.86-2.11 5.83-2.51 2.78-1.16 3.35-1.36 3.73-1.36.08 0 .27.02.39.12.1.08.13.19.14.27-.01.06.01.24 0 .38z',
  refresh: 'M17.65 6.35A7.958 7.958 0 0 0 12 4c-4.42 0-7.99 3.58-7.99 8s3.57 8 7.99 8c3.73 0 6.84-2.55 7.73-6h-2.08A5.99 5.99 0 0 1 12 18c-3.31 0-6-2.69-6-6s2.69-6 6-6c1.66 0 3.14.69 4.22 1.78L13 11h7V4l-2.35 2.35z',
  sun: 'M12 7a5 5 0 1 0 0 10 5 5 0 0 0 0-10Zm0-5a1 1 0 0 1 1 1v2a1 1 0 1 1-2 0V3a1 1 0 0 1 1-1Zm0 17a1 1 0 0 1 1 1v2a1 1 0 1 1-2 0v-2a1 1 0 0 1 1-1ZM4.22 4.22a1 1 0 0 1 1.42 0l1.42 1.42a1 1 0 0 1-1.42 1.42L4.22 5.64a1 1 0 0 1 0-1.42Zm12.72 12.72a1 1 0 0 1 1.42 0l1.42 1.42a1 1 0 0 1-1.42 1.42l-1.42-1.42a1 1 0 0 1 0-1.42ZM2 12a1 1 0 0 1 1-1h2a1 1 0 1 1 0 2H3a1 1 0 0 1-1-1Zm17 0a1 1 0 0 1 1-1h2a1 1 0 1 1 0 2h-2a1 1 0 0 1-1-1ZM5.64 18.36a1 1 0 0 1 0-1.42l1.42-1.42a1 1 0 0 1 1.42 1.42l-1.42 1.42a1 1 0 0 1-1.42 0Zm12.72-12.72a1 1 0 0 1 0-1.42l1.42-1.42a1 1 0 1 1 1.42 1.42l-1.42 1.42a1 1 0 0 1-1.42 0Z',
  moon: 'M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79Z',
};

function AppIcon({ name, size = 22 }) {
  const d = ICONS[name] || ICONS.tools;
  return <svg width={size} height={size} viewBox="0 0 24 24" fill="currentColor"><path d={d} /></svg>;
}

function Loading() {
  return <div className="loading"><div className="spinner" /> Loading...</div>;
}

/* ========== SessionAiDialog Component ========== */
function SessionAiDialog({ session, profiles, busy, onClose, onApply }) {
  const [profileId, setProfileId] = useState(session?.account_or_profile_id || profiles[0]?.id || '');
  const selectedProfile = profiles.find(profile => profile.id === profileId) || profiles[0];
  const [model, setModel] = useState(session?.model || selectedProfile?.models?.[0]?.model_id || '');

  useEffect(() => {
    setProfileId(session?.account_or_profile_id || profiles[0]?.id || '');
  }, [profiles, session]);

  useEffect(() => {
    const profile = profiles.find(item => item.id === profileId);
    if (profile && !profile.models?.some(item => item.model_id === model)) {
      setModel(profile.models?.[0]?.model_id || '');
    }
  }, [model, profileId, profiles]);

  const selected = selectedProfile?.models?.find(item => item.model_id === model);

  return <Dialog labelId="session-ai-title" onClose={onClose}>
    <form onSubmit={event => {
      event.preventDefault();
      if (profileId && model && session) {
        onApply({ sessionId: session.id, profileId, model });
      }
    }}>
      <h3 id="session-ai-title">Change AI selection</h3>
      <p style={{fontSize:13,color:'var(--muted)',marginBottom:16}}>
        Changes apply only to <b>{session?.name || session?.id?.slice(0, 8)}</b>.
      </p>
      {profiles.length > 0 ? <>
        <Field label="Custom profile">
          <select className="form-select" value={profileId} onChange={event => setProfileId(event.target.value)}>
            {profiles.map(p => <option key={p.id} value={p.id}>{p.alias} · {p.endpoint}</option>)}
          </select>
        </Field>
        <Field label="Exact model">
          <select className="form-select" value={model} onChange={event => setModel(event.target.value)}>
            {(selectedProfile?.models || []).map(item =>
              <option key={item.model_id} value={item.model_id}>{item.model_id} — {modelReadiness(item)}</option>
            )}
          </select>
        </Field>
        <div style={{margin:'12px 0',fontSize:12,color:'var(--ink-soft)'}}>
          <Status value={selected?.probe_status || 'unprobed'} />
          <span style={{marginLeft:8}}>
            {modelReadiness(selected)}
          </span>
        </div>
        <div className="modal-actions">
          <Button onClick={onClose}>Cancel</Button>
          <Button tone="primary" type="submit" disabled={busy || !model}>Apply Custom model</Button>
        </div>
      </> : <Empty>Create and discover a Custom profile before selecting session AI.</Empty>}
    </form>
  </Dialog>;
}

/* ========== ProfileEditor Component ========== */
function ProfileEditor({ profile, isNew, busy, onBack, onSaved, onError }) {
  const [alias, setAlias] = useState(profile?.alias || '');
  const [endpoint, setEndpoint] = useState(profile?.endpoint || '');
  const [protocol, setProtocol] = useState(profile?.protocol || 'openai_chat_completions');
  const [keyAction, setKeyAction] = useState('keep');
  const [apiKey, setApiKey] = useState('');
  const [safeHeaders, setSafeHeaders] = useState('');
  const [secretHeaders, setSecretHeaders] = useState('');
  const [clearSecretHeaders, setClearSecretHeaders] = useState(false);
  const [keepCredential, setKeepCredential] = useState(false);
  const [keepSafeHeaders, setKeepSafeHeaders] = useState(false);
  const [keepSecretHeaders, setKeepSecretHeaders] = useState(false);

  useEffect(() => {
    if (profile) {
      setAlias(profile.alias || '');
      setEndpoint(profile.endpoint || '');
      setProtocol(profile.protocol || 'openai_chat_completions');
      setKeyAction('keep');
      setApiKey('');
      setSafeHeaders('');
      setSecretHeaders('');
      setClearSecretHeaders(false);
      setKeepCredential(false);
      setKeepSafeHeaders(false);
      setKeepSecretHeaders(false);
    } else {
      setAlias('');
      setEndpoint('');
      setProtocol('openai_chat_completions');
      setKeyAction('replace');
      setApiKey('');
      setSafeHeaders('');
      setSecretHeaders('');
      setClearSecretHeaders(false);
      setKeepCredential(false);
      setKeepSafeHeaders(false);
      setKeepSecretHeaders(false);
    }
  }, [profile, isNew]);

  const endpointChanged = Boolean(profile && endpoint.trim() !== profile.endpoint);
  const configuredKey = Boolean(profile?.api_key_configured);
  const configuredHeaderCount = (profile?.header_names || []).length;

  const safeHeaderHint = endpointChanged
    ? 'Blank clears old safe headers. Supply JSON here to bind replacements to the new endpoint.'
    : 'Blank preserves current safe headers. Use {} to clear them.';
  const secretHeaderHint = endpointChanged
    ? 'Blank clears old secret headers. Supply replacement JSON for the new endpoint; values stay write-only.'
    : 'Blank preserves current secret headers. Values are write-only; only names may be displayed later.';

  const handleSubmit = async (event) => {
    if (event) event.preventDefault();
    let parsedSafe;
    let parsedSecret;
    try {
      parsedSafe = parseHeaderObject(safeHeaders, 'Safe');
      parsedSecret = parseHeaderObject(secretHeaders, 'Secret');
    } catch (err) {
      onError(err.message);
      return;
    }
    if (!alias.trim()) {
      onError('Alias is required');
      return;
    }
    if (!endpoint.trim()) {
      onError('Endpoint URL is required');
      return;
    }

    const body = {
      action: isNew ? 'create' : 'edit',
      alias: alias.trim(),
      endpoint: endpoint.trim(),
      protocol,
      ...(profile ? { profile_id: profile.id } : {}),
      ...(parsedSafe !== undefined ? { headers: parsedSafe } : {}),
      ...(parsedSecret !== undefined ? { secret_headers: parsedSecret } : {}),
      ...(keyAction === 'replace' && apiKey.trim() ? { api_key: apiKey.trim() } : {}),
      remove_api_key: !isNew && keyAction === 'remove',
      clear_secret_headers: clearSecretHeaders,
      keep_credential: endpointChanged && keepCredential,
      keep_safe_headers: endpointChanged && keepSafeHeaders,
      keep_secret_headers: endpointChanged && keepSecretHeaders,
    };

    try {
      await managerPost('provider-custom', body);
      onSaved(isNew ? 'Created Custom profile' : 'Updated Custom profile');
    } catch (err) {
      onError(err.message);
    }
  };

  return <div className="sub-page">
    <div className="sub-header">
      <button className="back-btn" onClick={onBack}>←</button>
      <h2>{isNew ? 'New Custom profile' : `Edit ${profile?.alias || 'Profile'}`}</h2>
    </div>
    <div className="sub-content">
      <p style={{fontSize:13,color:'var(--muted)',marginBottom:16}}>
        Secret values are submitted only to xiaod and never rendered back into this page. Values remain write-only.
      </p>
      {profile && <div className="notice info" style={{marginBottom:16}}>
        <span className="notice-dot" />
        <span>{configuredKey ? 'API key configured' : 'No API key configured'} · {configuredHeaderCount} configured header name{configuredHeaderCount === 1 ? '' : 's'}</span>
      </div>}
      <div className="detail-card">
        <form onSubmit={handleSubmit}>
          <Field label="Alias"><input className="form-input" id="pe-alias" required maxLength={80} value={alias} onChange={e => setAlias(e.target.value)} placeholder="studio-local" /></Field>
          <Field label="Endpoint URL"><input className="form-input" id="pe-endpoint" required type="url" value={endpoint} onChange={e => setEndpoint(e.target.value)} placeholder="https://api.example.com/v1" /></Field>
          <Field label="Protocol"><select className="form-select" id="pe-protocol" value={protocol} onChange={e => setProtocol(e.target.value)}>
            <option value="openai_chat_completions">OpenAI Chat Completions</option>
            <option value="openai_responses">OpenAI Responses</option>
          </select></Field>
          <Field label="API key">
            <select className="form-select" value={keyAction} onChange={e => setKeyAction(e.target.value)}>
              <option value="keep">{profile ? 'Keep current key' : 'No API key'}</option>
              <option value="replace">Set replacement key</option>
              {profile && <option value="remove">Remove key</option>}
            </select>
          </Field>
          {keyAction === 'replace' && <Field label="Replacement API key" wide>
            <input className="form-input" type="password" autoComplete="new-password" id="pe-key" value={apiKey} onChange={e => setApiKey(e.target.value)} placeholder="sk-..." />
          </Field>}
          <Field label="Safe headers" hint={safeHeaderHint} wide>
            <textarea className="form-textarea" id="pe-safe-headers" rows="3" value={safeHeaders} onChange={e => setSafeHeaders(e.target.value)} placeholder='{"X-Workspace":"personal"}' />
          </Field>
          <Field label="Secret headers" hint={secretHeaderHint} wide>
            <textarea className="form-textarea" id="pe-secret-headers" rows="3" value={secretHeaders} onChange={e => setSecretHeaders(e.target.value)} placeholder='{"Authorization":"Bearer …"}' />
          </Field>
          {profile && !endpointChanged && <div className="toggle-row">
            <label>Clear stored secret headers</label>
            <button type="button" className={'toggle-switch' + (clearSecretHeaders ? ' on' : '')} onClick={() => setClearSecretHeaders(!clearSecretHeaders)} />
          </div>}
          {endpointChanged && <div className="notice warn" style={{flexDirection:'column',alignItems:'flex-start',gap:8}}>
            <b>Endpoint trust boundary changed</b>
            <p style={{margin:0,fontSize:12}}>Model discovery and probes are invalidated. The default clears old credentials and headers; replacements entered above are committed in this same patch.</p>
            {configuredKey && <label style={{fontSize:13,display:'flex',alignItems:'center',gap:6}}>
              <input type="checkbox" checked={keepCredential} onChange={e => setKeepCredential(e.target.checked)} />
              Explicitly retain existing API key
            </label>}
            {configuredHeaderCount > 0 && <>
              <label style={{fontSize:13,display:'flex',alignItems:'center',gap:6}}>
                <input type="checkbox" checked={keepSafeHeaders} onChange={e => setKeepSafeHeaders(e.target.checked)} />
                Explicitly retain existing safe headers
              </label>
              <label style={{fontSize:13,display:'flex',alignItems:'center',gap:6}}>
                <input type="checkbox" checked={keepSecretHeaders} onChange={e => setKeepSecretHeaders(e.target.checked)} />
                Explicitly retain existing secret headers
              </label>
            </>}
          </div>}
          <div className="modal-actions" style={{marginTop:20}}>
            <Button onClick={onBack}>Cancel</Button>
            <Button tone="primary" type="submit" disabled={busy}>
              {isNew ? 'Create isolated profile' : 'Commit profile update'}
            </Button>
          </div>
        </form>
      </div>
    </div>
  </div>;
}

/* ========== Main App Component ========== */
function App() {
  const [tab, setTab] = useState('explore');
  const [sub, setSub] = useState(null);
  const [subArg, setSubArg] = useState(null);
  const [data, setData] = useState({});
  const [busy, setBusy] = useState(false);
  const [toastMsg, setToastMsg] = useState(null);
  const [loading, setLoading] = useState({});
  const [sessionAiDialogTarget, setSessionAiDialogTarget] = useState(null);
  const [refreshing, setRefreshing] = useState(false);

  const [themePreference, setThemePreference] = useState(() => {
    try {
      return localStorage.getItem('xiao-theme-pref') || 'system';
    } catch {
      return 'system';
    }
  });

  const [appliedTheme, setAppliedTheme] = useState('light');

  useEffect(() => {
    const media = window.matchMedia('(prefers-color-scheme: dark)');
    const updateTheme = () => {
      const resolved = themePreference === 'system'
        ? (media.matches ? 'dark' : 'light')
        : themePreference;
      setAppliedTheme(resolved);
      document.documentElement.setAttribute('data-theme', resolved);
      try {
        localStorage.setItem('xiao-theme-pref', themePreference);
      } catch {}
    };

    updateTheme();
    media.addEventListener('change', updateTheme);
    return () => media.removeEventListener('change', updateTheme);
  }, [themePreference]);

  const cycleTheme = useCallback(() => {
    setThemePreference(prev => {
      if (prev === 'system') return 'dark';
      if (prev === 'dark') return 'light';
      return 'system';
    });
  }, []);

  const toast = useCallback((msg, type = 'info') => {
    setToastMsg({ msg, type });
    setTimeout(() => setToastMsg(null), 3500);
  }, []);

  const load = useCallback(async (key, query) => {
    const resource = resourceForView(key);
    setLoading(prev => ({ ...prev, [key]: true }));
    try {
      const result = await managerGet(resource, query || {});
      setData(prev => ({ ...prev, [key]: result }));
    } catch (e) { console.warn('load', key, e.message); }
    setLoading(prev => ({ ...prev, [key]: false }));
  }, []);

  const post = useCallback(async (resource, body) => {
    setBusy(true);
    try {
      const result = await managerPost(resource, body);
      return result;
    } catch (e) { toast(e.message, 'bad'); throw e; }
    finally { setBusy(false); }
  }, [toast]);

  const lastBackPressRef = useRef(0);

  const nav = useCallback((page, arg) => {
    setSub(page);
    setSubArg(arg || null);
    window.scrollTo(0, 0);
    try {
      window.history.pushState({ tab, sub: page, subArg: arg || null }, '', '#' + page);
    } catch {}
  }, [tab]);

  const selectTab = useCallback((newTab) => {
    setTab(newTab);
    setSub(null);
    setSubArg(null);
    window.scrollTo(0, 0);
    try {
      window.history.pushState({ tab: newTab, sub: null, subArg: null }, '', '#' + newTab);
    } catch {}
  }, []);

  const back = useCallback(() => {
    try {
      window.history.back();
    } catch {
      setSub(null);
      setSubArg(null);
    }
  }, []);

  useEffect(() => {
    try {
      window.history.replaceState({ tab: 'explore', sub: null, subArg: null, isRoot: true }, '', '#explore');
      window.history.pushState({ tab: 'explore', sub: null, subArg: null, isMain: true }, '', '#explore');
    } catch {}

    const handlePopState = (event) => {
      const state = event.state;
      if (!state || state.isRoot) {
        const now = Date.now();
        if (now - lastBackPressRef.current < 2000) {
          window.history.back();
          return;
        }
        lastBackPressRef.current = now;
        toast('Tekan kembali sekali lagi untuk keluar', 'info');
        try {
          window.history.pushState({ tab: 'explore', sub: null, subArg: null, isMain: true }, '', '#explore');
        } catch {}
        setTab('explore');
        setSub(null);
        setSubArg(null);
        return;
      }

      if (state.tab) setTab(state.tab);
      setSub(state.sub || null);
      setSubArg(state.subArg || null);
      window.scrollTo(0, 0);
    };

    window.addEventListener('popstate', handlePopState);
    return () => window.removeEventListener('popstate', handlePopState);
  }, [toast]);

  const handleManualRefresh = useCallback(async () => {
    if (refreshing) return;
    setRefreshing(true);
    const startTime = Date.now();
    try {
      const resourcesToReload = sub
        ? (RELOAD_MAP[sub] || [sub])
        : ['dashboard', 'providers', 'setup', 'agent'];
      await Promise.all(resourcesToReload.map(r => load(r)));
      const elapsed = Date.now() - startTime;
      if (elapsed < 650) {
        await new Promise(r => setTimeout(r, 650 - elapsed));
      }
      toast('Data berhasil diperbarui', 'ok');
    } catch {
      toast('Gagal memperbarui data', 'bad');
    } finally {
      setRefreshing(false);
    }
  }, [refreshing, sub, load, toast]);

  useEffect(() => {
    load('dashboard');
    load('providers');
    load('setup');
    load('agent');
  }, [load]);

  const dash = data.dashboard;
  const health = dash?.health;
  const ai = dash?.current_ai;
  const counts = dash?.counts;
  const profiles = data.providers?.custom_profiles || [];
  const tg = data.setup;

  const handleApplySessionAi = async ({ sessionId, profileId, model }) => {
    try {
      await managerPost('sessions', {
        action: 'ai_config',
        session_id: sessionId,
        provider: 'custom',
        account_or_profile_id: profileId,
        model,
      });
      setSessionAiDialogTarget(null);
      toast('AI selection updated', 'ok');
      load('dashboard');
      load('sessions');
    } catch (e) {
      toast(e.message, 'bad');
    }
  };

  /* ========== Toast ========== */
  function ToastUI() {
    if (!toastMsg) return null;
    return <div className={`notice ${toastMsg.type}`} style={{position:'fixed',top:'calc(56px + var(--status-bar-inset) + 14px)',left:'50%',transform:'translateX(-50%)',zIndex:100,maxWidth:400,width:'90%',boxShadow:'0 8px 24px rgba(0,0,0,.25)'}}>
      <span className="notice-dot" />{toastMsg.msg}
    </div>;
  }

  /* ========== Sub Header ========== */
  function SubHeader({ title, actions }) {
    return <div className="sub-header">
      <button className="back-btn" onClick={back} aria-label="Back">←</button>
      <h2>{title}</h2>
      {actions}
    </div>;
  }

  /* ========== Settings Item Component ========== */
  function SettingsItem({ icon, bg, color, title, right, onClick }) {
    return <div className="settings-item" onClick={onClick}>
      <div className="settings-icon" style={{ background: bg, color }}><AppIcon name={icon} size={20} /></div>
      <div className="settings-body"><h4>{title}</h4></div>
      {right && <span className="settings-right">{right}</span>}
      <span className="card-chevron">›</span>
    </div>;
  }

  /* ===================================================
     VIEW: Explore (Dashboard)
     =================================================== */
  function ExploreView() {
    const gateway = health?.gateway || 'unknown';
    const gsc = classForStatus(gateway);
    return <>
      <div className="hero">
        <div className="hero-avatar">
          <img className="hero-logo" src="./xiao-logo.png" alt="Xiao logo" />
          <div className="edit-badge">✏</div>
        </div>
        <h2>Hi, Boss!</h2>
        <div className="model-badge" onClick={() => nav('models')}>
          <span className={`status-dot ${gsc}`} />
          {ai?.model || 'Select Model'} ›
        </div>
      </div>
      <div className="content">
        {counts && <div className="metrics-grid">
          <div className="metric"><div className="metric-value">{counts.sessions || 0}</div><div className="metric-label">Sessions</div></div>
          <div className="metric"><div className="metric-value">{counts.attachments || 0}</div><div className="metric-label">Attachments</div></div>
          <div className="metric"><div className="metric-value">{counts.skills || 0}</div><div className="metric-label">Skills</div></div>
        </div>}
        <div className="section">
          <div className="section-header">
            <span className="section-title">Essential</span>
          </div>
          <div className="card-group">
            <div className="card-item" onClick={() => { load('sessions'); nav('sessions'); }}>
              <div className="card-icon" style={{ background: 'var(--accent-soft)', color: 'var(--accent)' }}><AppIcon name="chat" size={22} /></div>
              <div className="card-body"><h4>Sessions</h4><p>{counts?.sessions || 0} total sessions</p></div>
              <span className="card-chevron">›</span>
            </div>
            <div className="card-item" onClick={() => { load('runs'); nav('runs'); }}>
              <div className="card-icon" style={{ background: 'var(--yellow-soft)', color: 'var(--yellow)' }}><AppIcon name="play" size={22} /></div>
              <div className="card-body"><h4>Agent Runs</h4><p>{counts?.runs || 0} active / historical runs</p></div>
              <span className="card-chevron">›</span>
            </div>
            <div className="card-item" onClick={() => { load('attachments'); nav('attachments'); }}>
              <div className="card-icon" style={{ background: 'var(--teal-soft)', color: 'var(--teal)' }}><AppIcon name="bolt" size={22} /></div>
              <div className="card-body"><h4>Attachments</h4><p>{counts?.attachments || 0} files stored</p></div>
              <span className="card-chevron">›</span>
            </div>
          </div>
        </div>
        <div className="section">
          <div className="section-header">
            <span className="section-title">System & Security</span>
          </div>
          <div className="card-group">
            <div className="card-item" onClick={() => { load('security'); nav('security'); }}>
              <div className="card-icon" style={{ background: 'var(--red-soft)', color: 'var(--red)' }}><AppIcon name="tools" size={22} /></div>
              <div className="card-body"><h4>Security</h4><p>Approvals, YOLO, and audit</p></div>
              <span className="card-chevron">›</span>
            </div>
            <div className="card-item" onClick={() => { load('diagnostics'); nav('diagnostics'); }}>
              <div className="card-icon" style={{ background: 'var(--purple-soft)', color: 'var(--purple)' }}><AppIcon name="spark" size={22} /></div>
              <div className="card-body"><h4>Diagnostics</h4><p>Independent health checks</p></div>
              <span className="card-chevron">›</span>
            </div>
          </div>
        </div>
      </div>
    </>;
  }

  /* ===================================================
     VIEW: Tools (Imagine)
     =================================================== */
  function ToolsView() {
    return <div className="content" style={{ paddingTop: 20 }}>
      <div className="section">
        <div className="section-header"><span className="section-title">Capabilities</span></div>
        <div className="card-group">
          <div className="card-item" onClick={() => { load('tools'); nav('tools'); }}>
            <div className="card-icon" style={{ background: 'var(--accent-soft)', color: 'var(--accent)' }}><AppIcon name="tools" size={22} /></div>
            <div className="card-body"><h4>Tools Surface</h4><p>Typed built-in and Termux capabilities</p></div>
            <span className="card-chevron">›</span>
          </div>
          <div className="card-item" onClick={() => { load('skills'); nav('skills'); }}>
            <div className="card-icon" style={{ background: 'var(--purple-soft)', color: 'var(--purple)' }}><AppIcon name="spark" size={22} /></div>
            <div className="card-body"><h4>Skills</h4><p>Learned workflows and instructions</p></div>
            <span className="card-chevron">›</span>
          </div>
          <div className="card-item" onClick={() => { load('memory'); nav('memory'); }}>
            <div className="card-icon" style={{ background: 'var(--teal-soft)', color: 'var(--teal)' }}><AppIcon name="brain" size={22} /></div>
            <div className="card-body"><h4>Memory</h4><p>Owner facts and preferences</p></div>
            <span className="card-chevron">›</span>
          </div>
        </div>
      </div>
      <div className="section">
        <div className="section-header"><span className="section-title">Environment</span></div>
        <div className="card-group">
          <div className="card-item" onClick={() => { load('runtime'); nav('runtime'); }}>
            <div className="card-icon" style={{ background: 'var(--yellow-soft)', color: 'var(--yellow)' }}><AppIcon name="settings" size={22} /></div>
            <div className="card-body"><h4>Runtime</h4><p>Device truth and execution environment</p></div>
            <span className="card-chevron">›</span>
          </div>
          <div className="card-item" onClick={() => { load('context'); nav('context'); }}>
            <div className="card-icon" style={{ background: 'var(--accent-soft)', color: 'var(--accent)' }}><AppIcon name="chat" size={22} /></div>
            <div className="card-body"><h4>Context</h4><p>Active bounded context engine</p></div>
            <span className="card-chevron">›</span>
          </div>
        </div>
      </div>
    </div>;
  }

  /* ===================================================
     VIEW: Settings
     =================================================== */
  function SettingsView() {
    return <div className="content" style={{ paddingTop: 8 }}>
      <div className="settings-section-title">Xiao</div>
      <div className="card-group">
        <SettingsItem icon="settings" bg="var(--accent-soft)" color="var(--accent)" title="Agent Config" onClick={() => { load('agent'); nav('agent'); }} />
        <SettingsItem icon="telegram" bg="var(--accent-soft)" color="var(--accent)" title="Telegram" right={(tg?.telegram?.enabled ?? tg?.enabled) ? 'Active' : 'Off'} onClick={() => { load('setup'); nav('telegram'); }} />
      </div>
      <div className="settings-section-title">AI Provider</div>
      <div className="card-group">
        <SettingsItem icon="plug" bg="var(--purple-soft)" color="var(--purple)" title="Custom Profiles" right={profiles.length + ' profiles'} onClick={() => { load('providers'); nav('profiles'); }} />
        <SettingsItem icon="spark" bg="var(--pink-soft, #fce4ec)" color="var(--pink)" title="LLM Model" right={ai?.model || '-'} onClick={() => { load('providers'); nav('models'); }} />
      </div>
      <div className="settings-section-title">Appearance</div>
      <div className="card-group">
        <div className="settings-item" onClick={cycleTheme} style={{ cursor: 'pointer' }}>
          <div className="settings-icon" style={{ background: appliedTheme === 'dark' ? 'var(--purple-soft)' : 'var(--yellow-soft)', color: appliedTheme === 'dark' ? 'var(--purple)' : 'var(--yellow)' }}>
            <AppIcon name={appliedTheme === 'dark' ? 'moon' : 'sun'} size={19} />
          </div>
          <div className="settings-body">
            <h4>Theme: {themePreference === 'system' ? 'System' : (themePreference === 'dark' ? 'Dark' : 'Light')}</h4>
            <p>{themePreference === 'system' ? `Following OS (${appliedTheme})` : `${appliedTheme} mode`}</p>
          </div>
          <span className="settings-right">{themePreference.toUpperCase()}</span>
        </div>
      </div>
      <div className="settings-section-title">App</div>
      <div className="card-group">
        <SettingsItem icon="info" bg="var(--teal-soft)" color="var(--teal)" title="About Xiao" onClick={() => nav('about')} />
        <SettingsItem icon="chat" bg="var(--line)" color="var(--ink-soft)" title="Daemon Logs" onClick={() => { load('logs'); nav('logs'); }} />
      </div>
    </div>;
  }

  /* ===================================================
     SUB-PAGES
     =================================================== */

  /* Sessions */
  function SessionsPage() {
    const d = data.sessions;
    if (loading.sessions && !d) return <div className="sub-page"><SubHeader title="Sessions" /><Loading /></div>;
    const items = d?.items || d?.sessions || [];
    const totalPages = d?.pages || d?.total_pages || 1;
    const curPage = d?.page || 1;
    return <div className="sub-page">
      <SubHeader title="Sessions" actions={<Button tone="primary" onClick={async () => {
        try {
          const s = await post('sessions', { action: 'new', name: 'New Session' });
          toast('Session created', 'ok');
          load('sessions');
          load('dashboard');
        } catch {}
      }}>+ New</Button>} />
      <div className="sub-content">
        {items.length === 0 ? <Empty>No sessions found</Empty> : <>
          <div className="card-group">{items.map((s, i) =>
            <div className="card-item" key={s.id || i} onClick={() => nav('session-detail', { session: s })}>
              <div className="card-body">
                <h4>{s.name || 'Untitled'}</h4>
                <p>{s.model || '-'} · {s.message_count || 0} messages</p>
              </div>
              <span className="card-chevron">›</span>
            </div>
          )}</div>
          {totalPages > 1 && <Pagination page={curPage} pages={totalPages} onPage={p => load('sessions', { page: p })} />}
        </>}
      </div>
    </div>;
  }

  /* Session Detail */
  function SessionDetailPage() {
    const s = subArg?.session;
    if (!s) return <div className="sub-page"><SubHeader title="Session" /><Empty>No session selected</Empty></div>;
    return <div className="sub-page">
      <SubHeader title={s.name || 'Session Detail'} actions={
        <div style={{display:'flex',gap:6}}>
          <Button onClick={() => setSessionAiDialogTarget(s)}>AI Config</Button>
          <Button tone="danger" onClick={async () => {
            if (!confirm('Delete this session?')) return;
            try {
              await post('sessions', { action: 'delete', session_id: s.id });
              toast('Deleted', 'ok');
              back();
              load('sessions');
              load('dashboard');
            } catch {}
          }}>✕</Button>
        </div>
      } />
      <div className="sub-content">
        <div className="detail-card">
          <DL label="Session ID" mono>{s.id}</DL>
          <DL label="Model">{s.model || '-'}</DL>
          <DL label="Provider">{s.provider || '-'}</DL>
          <DL label="YOLO Mode"><Status value={s.yolo_mode ? 'enabled' : 'disabled'} /></DL>
          <DL label="Messages">{s.message_count || 0}</DL>
          <DL label="Created">{s.created_at || '-'}</DL>
        </div>
      </div>
    </div>;
  }

  /* Models */
  function ModelsPage() {
    const allModels = [];
    profiles.forEach(p => (p.models || []).forEach(m => allModels.push({ ...m, profileAlias: p.alias, profileId: p.id })));
    const currentModel = ai?.model;

    const handleOverride = async (profileId, modelId, capability, override) => {
      try {
        await post('provider-custom', {
          action: 'capability_override',
          profile_id: profileId,
          model: modelId,
          capability,
          owner_override: override,
        });
        toast(`Capability ${capability} set to ${override}`, 'ok');
        load('providers');
      } catch (e) {
        toast(e.message, 'bad');
      }
    };

    const handleProbe = async (profileId, modelId) => {
      try {
        await post('provider-custom', { action: 'probe', profile_id: profileId, model: modelId });
        toast('Model probe completed', 'ok');
        load('providers');
      } catch (e) {
        toast(e.message, 'bad');
      }
    };

    return <div className="sub-page">
      <SubHeader title="LLM Models & Capabilities" />
      <div className="sub-content">
        {allModels.length === 0 ? <Empty>No models discovered. Add a Custom Profile first.</Empty> :
          <div className="card-group">{allModels.map((m, i) =>
            <div className={'model-item' + (m.model_id === currentModel ? ' selected' : '')} key={m.model_id || i}>
              <div style={{ flex: 1, marginRight: 8 }}>
                <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
                  <h4>{m.model_id}</h4>
                  {m.model_id === currentModel && <span className="tag tag-ok">ACTIVE</span>}
                </div>
                <p>{m.profileAlias} · Readiness: <b>{modelReadiness(m)}</b></p>
                <div style={{ display: 'flex', flexWrap: 'wrap', gap: 10, marginTop: 8, fontSize: 12 }}>
                  <div>
                    <span>Vision: </span>
                    <select
                      className="form-select"
                      style={{ display: 'inline-block', width: 'auto', padding: '2px 6px', fontSize: 12 }}
                      value={m.vision_override || 'auto'}
                      onChange={e => handleOverride(m.profileId, m.model_id, 'vision', e.target.value)}
                    >
                      <option value="auto">Auto ({m.vision_state || 'unknown'})</option>
                      <option value="force_supported">Force Supported</option>
                      <option value="force_unsupported">Force Unsupported</option>
                    </select>
                  </div>
                  <div>
                    <span>File Input: </span>
                    <select
                      className="form-select"
                      style={{ display: 'inline-block', width: 'auto', padding: '2px 6px', fontSize: 12 }}
                      value={m.file_input_override || 'auto'}
                      onChange={e => handleOverride(m.profileId, m.model_id, 'file_input', e.target.value)}
                    >
                      <option value="auto">Auto ({m.file_input_state || 'unknown'})</option>
                      <option value="force_supported">Force Supported</option>
                      <option value="force_unsupported">Force Unsupported</option>
                    </select>
                  </div>
                </div>
              </div>
              <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
                <Button btn-sm onClick={() => handleProbe(m.profileId, m.model_id)}>Probe</Button>
                <Button tone="primary" btn-sm onClick={async () => {
                  try {
                    const sid = ai?.session_id || (data.sessions?.items || data.sessions?.sessions || [])[0]?.id;
                    if (sid) {
                      await post('sessions', { action: 'ai_config', session_id: sid, provider: 'custom', account_or_profile_id: m.profileId, model: m.model_id });
                      toast('Model set to ' + m.model_id, 'ok');
                      load('dashboard');
                    } else {
                      toast('No active session', 'warn');
                    }
                  } catch (e) {
                    toast(e.message, 'bad');
                  }
                }}>Select</Button>
              </div>
            </div>
          )}</div>}
      </div>
    </div>;
  }

  /* Memory */
  function MemoryPage() {
    const d = data.memory;
    if (loading.memory && !d) return <div className="sub-page"><SubHeader title="Memory" /><Loading /></div>;
    const items = d?.items || [];
    return <div className="sub-page">
      <SubHeader title="Memory" actions={<Button onClick={async () => {
        try {
          await post('memory', { action: 'reconcile' });
          toast('Memory reconciled', 'ok');
          load('memory');
        } catch {}
      }}>Reconcile</Button>} />
      <div className="sub-content">
        {items.length === 0 ? <Empty>No memories stored yet</Empty> :
          items.map((m, i) => <div className="detail-card" key={m.id || i} style={{marginBottom:10}}>
            <div style={{display:'flex',justifyContent:'space-between',alignItems:'start'}}>
              <div>
                <div className="detail-label">{m.scope || m.category || 'memory'} · {m.category || ''}</div>
                <div style={{fontSize:14,fontWeight:500}}>{m.content || m.value || m.key || '-'}</div>
                {m.key && <div style={{fontSize:12,color:'var(--muted)',marginTop:4,fontFamily:'var(--font-mono)'}}>{m.key}</div>}
              </div>
              <Button tone="danger" onClick={async () => {
                try {
                  await post('memory', { action: 'delete', scope: m.scope || 'user', category: m.category || 'general', key: m.key });
                  toast('Forgotten', 'ok');
                  load('memory');
                } catch {}
              }}>✕</Button>
            </div>
          </div>)}
      </div>
    </div>;
  }

  /* Skills */
  function SkillsPage() {
    const d = data.skills;
    if (loading.skills && !d) return <div className="sub-page"><SubHeader title="Skills" /><Loading /></div>;
    const items = d?.items || d?.skills || [];
    return <div className="sub-page">
      <SubHeader title="Skills" actions={<Button onClick={async () => {
        try {
          await post('skills', { action: 'refresh' });
          toast('Skills refreshed', 'ok');
          load('skills');
        } catch {}
      }}>Refresh</Button>} />
      <div className="sub-content">
        {items.length === 0 ? <Empty>No skills installed</Empty> :
          items.map((s, i) => <div className="detail-card" key={s.id || i} style={{marginBottom:10}}>
            <div style={{display:'flex',justifyContent:'space-between',alignItems:'start'}}>
              <div>
                <div style={{fontSize:15,fontWeight:600}}>{s.name || '-'}</div>
                <div style={{fontSize:13,color:'var(--ink-soft)',marginTop:4}}>{s.summary || s.description || '-'}</div>
              </div>
              <Button tone="danger" onClick={async () => {
                if (!confirm(`Delete skill ${s.name}?`)) return;
                try {
                  await post('skills', { action: 'delete', skill_id: s.id || s.name });
                  toast('Skill deleted', 'ok');
                  load('skills');
                } catch {}
              }}>✕</Button>
            </div>
          </div>)}
      </div>
    </div>;
  }

  /* Tools */
  function ToolsListPage() {
    const d = data.tools;
    if (loading.tools && !d) return <div className="sub-page"><SubHeader title="Tools" /><Loading /></div>;
    const items = d?.items || d?.tools || [];
    return <div className="sub-page">
      <SubHeader title="Tools Surface" />
      <div className="sub-content">
        {items.length === 0 ? <Empty>No tools available</Empty> :
          items.map((t, i) => <div className="detail-card" key={t.name || i} style={{marginBottom:10}}>
            <div style={{display:'flex',justifyContent:'space-between',alignItems:'center'}}>
              <span style={{fontWeight:600,fontFamily:'var(--font-mono)'}}>{t.name}</span>
              <Status value={t.risk || 'safe'} />
            </div>
            <p style={{fontSize:13,color:'var(--muted)',marginTop:4}}>{t.description || '-'}</p>
          </div>)}
      </div>
    </div>;
  }

  /* Runs */
  function RunsPage() {
    const d = data.runs;
    if (loading.runs && !d) return <div className="sub-page"><SubHeader title="Agent Runs" /><Loading /></div>;
    const items = d?.items || d?.runs || [];
    return <div className="sub-page">
      <SubHeader title="Agent Runs" />
      <div className="sub-content">
        {items.length === 0 ? <Empty>No agent runs yet</Empty> :
          items.map((r, i) => <div className="detail-card" key={r.id || i} style={{marginBottom:12}}>
            <div style={{display:'flex',justifyContent:'space-between',alignItems:'center',marginBottom:6}}>
              <Status value={r.status} />
              <span style={{fontSize:12,color:'var(--muted)'}}>{r.created_at || ''}</span>
            </div>
            <div style={{fontSize:13,fontWeight:600}}>{r.provider || '-'} · {r.model || '-'}</div>
            {r.task && <div style={{fontSize:13,color:'var(--ink)',marginTop:4}}>{r.task}</div>}
            <div style={{fontSize:11,color:'var(--muted)',marginTop:4,fontFamily:'var(--font-mono)'}}>{r.id}</div>
            {r.timings && r.timings.length > 0 && <div className="timing-waterfall">
              <div style={{fontWeight:600,marginBottom:4}}>Timing Waterfall</div>
              {r.timings.map((t, idx) => <div className="timing-row" key={idx}>
                <span className="timing-label">{t.kind?.replace(/_/g, ' ')}</span>
                <span className="timing-value">{t.elapsed_ms} ms</span>
              </div>)}
            </div>}
            {r.status === 'running' && <div style={{marginTop:10}}><Button tone="danger" onClick={async () => {
              try {
                await post('runs', { action: 'cancel', run_id: r.id });
                toast('Cancelled', 'ok');
                load('runs');
              } catch {}
            }}>Cancel Run</Button></div>}
          </div>)}
      </div>
    </div>;
  }

  /* Attachments */
  function AttachmentsPage() {
    const d = data.attachments;
    if (loading.attachments && !d) return <div className="sub-page"><SubHeader title="Attachments" /><Loading /></div>;
    const items = d?.items || d?.attachments || [];
    return <div className="sub-page">
      <SubHeader title="Attachments" />
      <div className="sub-content">
        {items.length === 0 ? <Empty>No attachments</Empty> :
          items.map((a, i) => <div className="detail-card" key={a.id || i} style={{marginBottom:10}}>
            <div style={{display:'flex',justifyContent:'space-between',alignItems:'start'}}>
              <div>
                <div style={{fontSize:14,fontWeight:500}}>{a.name || a.filename || '-'}</div>
                <div style={{fontSize:12,color:'var(--muted)',marginTop:2}}>{a.mime || a.kind || '-'} · {a.status || ''}</div>
              </div>
              <Button tone="danger" onClick={async () => {
                try {
                  await post('attachments', { action: 'remove', attachment_id: a.id });
                  toast('Attachment removed', 'ok');
                  load('attachments');
                } catch {}
              }}>✕</Button>
            </div>
          </div>)}
      </div>
    </div>;
  }

  /* Agent Settings */
  function AgentPage() {
    const d = data.agent;
    if (loading.agent && !d) return <div className="sub-page"><SubHeader title="Agent Settings" /><Loading /></div>;
    const s = d?.settings || {};
    const fields = [
      { key: 'max_turns', label: 'Maximum Agent Turns', type: 'number' },
      { key: 'max_tool_calls', label: 'Maximum Tool Calls', type: 'number' },
      { key: 'max_runtime_seconds', label: 'Runtime Timeout (seconds)', type: 'number' },
      { key: 'max_no_progress_repeats', label: 'No-Progress Repeat Threshold', type: 'number' },
      { key: 'provider_streaming', label: 'Provider Streaming', type: 'toggle' },
      { key: 'parallel_readonly_tools', label: 'Parallel Read-Only Tools', type: 'toggle' },
      { key: 'max_parallel_readonly_tools', label: 'Max Parallel Read-Only', type: 'number' },
      { key: 'execution_plan_enabled', label: 'Structured Execution Plan', type: 'toggle' },
      { key: 'plan_cache_enabled', label: 'Plan Cache', type: 'toggle' },
      { key: 'background_learning', label: 'Background Learning', type: 'toggle' },
    ];

    const toggleField = async (key, label, currentVal) => {
      const newVal = !currentVal;
      setData(prev => ({
        ...prev,
        agent: {
          ...prev.agent,
          settings: { ...(prev.agent?.settings || {}), [key]: newVal }
        }
      }));
      try {
        await post('agent', { action: 'update', [key]: newVal });
        toast(label + ' ' + (newVal ? 'enabled' : 'disabled'), 'ok');
        load('agent');
      } catch {
        setData(prev => ({
          ...prev,
          agent: {
            ...prev.agent,
            settings: { ...(prev.agent?.settings || {}), [key]: currentVal }
          }
        }));
      }
    };

    return <div className="sub-page">
      <SubHeader title="Agent Settings" />
      <div className="sub-content">
        {d?.active_runs != null && <div className="notice info"><span className="notice-dot" />{d.active_runs} active runs</div>}
        <div className="detail-card">
          {fields.map(f => f.type === 'toggle' ?
            <div className="toggle-row" key={f.key} onClick={() => toggleField(f.key, f.label, s[f.key])}>
              <label>{f.label}</label>
              <button
                type="button"
                className={'toggle-switch' + (s[f.key] ? ' on' : '')}
                onClick={(e) => { e.stopPropagation(); toggleField(f.key, f.label, s[f.key]); }}
                aria-label={f.label}
              />
            </div> :
            <div className="form-group" key={f.key}>
              <label className="form-label">{f.label}</label>
              <input className="form-input" type="number" defaultValue={s[f.key] ?? ''} onBlur={async e => {
                const val = parseInt(e.target.value);
                if (isNaN(val) || val === s[f.key]) return;
                try { await post('agent', { action: 'update', [f.key]: val }); toast(f.label + ' updated', 'ok'); load('agent'); } catch {}
              }} />
            </div>
          )}
        </div>
      </div>
    </div>;
  }

  /* Runtime */
  function RuntimePage() {
    const d = data.runtime;
    if (loading.runtime && !d) return <div className="sub-page"><SubHeader title="Runtime" /><Loading /></div>;
    const env = d?.environment || {};
    const deps = d?.dependencies || [];
    return <div className="sub-page">
      <SubHeader title="Runtime" />
      <div className="sub-content">
        <div className="detail-card">
          <div style={{fontSize:16,fontWeight:700,marginBottom:12}}>Environment</div>
          <DL label="Platform">{env.platform || env.os || '-'}</DL>
          <DL label="Architecture">{env.arch || '-'}</DL>
          <DL label="Termux">{env.termux ? '✓ Active' : 'No'}</DL>
          <DL label="UID">{String(env.effective_uid ?? '-')}</DL>
          <DL label="SELinux">{env.selinux || '-'}</DL>
        </div>
        {deps.length > 0 && <div className="detail-card">
          <div style={{fontSize:16,fontWeight:700,marginBottom:12}}>Dependencies</div>
          {deps.map((dep, i) => <div key={i} style={{display:'flex',justifyContent:'space-between',padding:'8px 0',borderBottom:'1px solid var(--line)'}}>
            <span>{dep.name || dep.id || '-'}</span><Status value={dep.status || dep.version} />
          </div>)}
        </div>}
      </div>
    </div>;
  }

  /* Context */
  function ContextPage() {
    const d = data.context;
    if (loading.context && !d) return <div className="sub-page"><SubHeader title="Context" /><Loading /></div>;
    return <div className="sub-page">
      <SubHeader title="Context" />
      <div className="sub-content"><div className="detail-card">
        <pre style={{fontSize:12,fontFamily:'var(--font-mono)',whiteSpace:'pre-wrap',wordBreak:'break-all',maxHeight:500,overflow:'auto'}}>
          {typeof d === 'string' ? d : JSON.stringify(d, null, 2)}
        </pre>
      </div></div>
    </div>;
  }

  /* Security */
  function SecurityPage() {
    const d = data.security;
    if (loading.security && !d) return <div className="sub-page"><SubHeader title="Security" /><Loading /></div>;
    const approvals = d?.pending_approvals || d?.items || [];
    const audit = d?.audit || [];
    return <div className="sub-page">
      <SubHeader title="Security" />
      <div className="sub-content">
        {approvals.length > 0 ? <>
          <div style={{fontSize:16,fontWeight:700,marginBottom:12}}>Pending Approvals</div>
          {approvals.map((a, i) => <div className="detail-card" key={a.id || a.approval_id || i} style={{marginBottom:10}}>
            <div style={{fontSize:14,fontWeight:500}}>{a.description || a.command || a.tool || '-'}</div>
            <div style={{display:'flex',gap:8,marginTop:10}}>
              <Button tone="primary" onClick={async () => {
                try {
                  await post('security', { action: 'approve', approval_id: a.approval_id || a.id });
                  toast('Approved', 'ok');
                  load('security');
                } catch {}
              }}>Approve</Button>
              <Button tone="danger" onClick={async () => {
                try {
                  await post('security', { action: 'deny', approval_id: a.approval_id || a.id });
                  toast('Denied', 'ok');
                  load('security');
                } catch {}
              }}>Deny</Button>
            </div>
          </div>)}
        </> : <div className="notice ok"><span className="notice-dot" />No pending approvals</div>}
        {audit.length > 0 && <><div style={{fontSize:16,fontWeight:700,margin:'20px 0 12px'}}>Audit Log</div>
          <div className="detail-card">{audit.slice(0, 50).map((a, i) => <div className="log-entry" key={i}>
            <div className="log-time">{a.timestamp || a.created_at || ''}</div>
            <div>{a.action || a.description || a.event || '-'}</div>
          </div>)}</div>
        </>}
      </div>
    </div>;
  }

  /* Diagnostics */
  function DiagnosticsPage() {
    const d = data.diagnostics;
    if (loading.diagnostics && !d) return <div className="sub-page"><SubHeader title="Diagnostics" /><Loading /></div>;
    const checks = d?.checks || d?.items || [];
    return <div className="sub-page">
      <SubHeader title="Diagnostics" actions={<Button onClick={() => load('diagnostics')}>↻ Refresh</Button>} />
      <div className="sub-content">
        {d?.ran_at && <div style={{fontSize:12,color:'var(--muted)',marginBottom:12}}>Last run: {d.ran_at}</div>}
        {checks.length === 0 ? <Empty>No diagnostic checks</Empty> :
          checks.map((c, i) => <div className="detail-card" key={c.id || i} style={{marginBottom:10}}>
            <div style={{display:'flex',justifyContent:'space-between',alignItems:'center'}}>
              <div style={{fontSize:14,fontWeight:500}}>{c.name || c.id || '-'}</div>
              <Status value={c.status || c.state} />
            </div>
            {(c.evidence || c.detail) && <div style={{fontSize:12,color:'var(--ink-soft)',marginTop:6}}>{c.evidence || c.detail}</div>}
          </div>)}
      </div>
    </div>;
  }

  /* Logs */
  function LogsPage() {
    const d = data.logs;
    if (loading.logs && !d) return <div className="sub-page"><SubHeader title="Logs" /><Loading /></div>;
    const lines = d?.lines || (typeof d === 'string' ? d.split('\n') : []);
    return <div className="sub-page">
      <SubHeader title="Logs" actions={<Button onClick={() => load('logs')}>↻</Button>} />
      <div className="sub-content"><div className="detail-card">
        <pre style={{fontSize:11,fontFamily:'var(--font-mono)',whiteSpace:'pre-wrap',wordBreak:'break-all',maxHeight:600,overflow:'auto',lineHeight:1.6}}>
          {Array.isArray(lines) ? lines.join('\n') : String(d || 'No log data')}
        </pre>
      </div></div>
    </div>;
  }

  /* Telegram */
  function TelegramPage() {
    const d = data.setup;
    if (loading.setup && !d) return <div className="sub-page"><SubHeader title="Telegram" /><Loading /></div>;
    const tgData = d?.telegram || d || {};

    const toggleTelegram = async () => {
      const currentVal = !!tgData.enabled;
      const newVal = !currentVal;
      setData(prev => {
        const prevSetup = prev.setup || {};
        const prevTg = prevSetup.telegram || prevSetup;
        return {
          ...prev,
          setup: {
            ...prevSetup,
            telegram: {
              ...prevTg,
              enabled: newVal
            },
            enabled: newVal
          }
        };
      });
      try {
        await post('telegram', { action: 'configure', enabled: newVal });
        toast('Telegram ' + (newVal ? 'enabled' : 'disabled'), 'ok');
        load('setup');
      } catch {
        setData(prev => {
          const prevSetup = prev.setup || {};
          const prevTg = prevSetup.telegram || prevSetup;
          return {
            ...prev,
            setup: {
              ...prevSetup,
              telegram: {
                ...prevTg,
                enabled: currentVal
              },
              enabled: currentVal
            }
          };
        });
        toast('Gagal mengubah status Telegram', 'bad');
      }
    };

    return <div className="sub-page">
      <SubHeader title="Telegram" />
      <div className="sub-content">
        <div className="detail-card">
          <div className="toggle-row" onClick={toggleTelegram}>
            <label>Telegram Enabled</label>
            <button
              type="button"
              className={'toggle-switch' + (tgData.enabled ? ' on' : '')}
              onClick={(e) => { e.stopPropagation(); toggleTelegram(); }}
              aria-label="Toggle Telegram"
            />
          </div>
          <DL label="Owner State"><Status value={tgData.owner_state} /></DL>
          {tgData.owner_user_id && <DL label="Owner User ID">{tgData.owner_user_id}</DL>}
          <DL label="Token Configured">{tgData.token_configured ? '✓ Yes' : '✗ No'}</DL>
          {tgData.bot && <DL label="Bot">@{tgData.bot.username || tgData.bot.first_name || '-'}</DL>}
          {tgData.allowed_chat_ids?.length > 0 && <DL label="Allowed Chats">{tgData.allowed_chat_ids.join(', ')}</DL>}
        </div>
        <div className="detail-card">
          <div style={{fontSize:16,fontWeight:700,marginBottom:12}}>Update Bot Token</div>
          <Field label="Bot Token"><input className="form-input" type="password" id="tg-token" placeholder="123456:ABC-DEF..." /></Field>
          <Field label="Owner User ID"><input className="form-input" type="number" id="tg-owner" defaultValue={tgData.owner_user_id || ''} /></Field>
          <div style={{display:'flex',gap:8}}>
            <Button tone="primary" onClick={async () => {
              const token = document.getElementById('tg-token')?.value;
              const owner = parseInt(document.getElementById('tg-owner')?.value);
              try {
                const body = { action: 'configure', confirm_owner_change: true };
                if (token) body.token = token;
                if (!isNaN(owner)) body.owner_user_id = owner;
                await post('telegram', body);
                toast('Updated', 'ok');
                load('setup');
              } catch {}
            }}>Save</Button>
            <Button onClick={async () => {
              try {
                const token = document.getElementById('tg-token')?.value;
                await post('telegram', { action: 'test', token: token || undefined });
                toast('Connection OK!', 'ok');
              } catch {}
            }}>Test Connection</Button>
          </div>
        </div>
      </div>
    </div>;
  }

  /* Custom Profiles */
  function ProfilesPage() {
    if (loading.providers && !data.providers) return <div className="sub-page"><SubHeader title="Custom AI Profiles" /><Loading /></div>;
    return <div className="sub-page">
      <SubHeader title="Custom AI Profiles" actions={<Button tone="primary" onClick={() => nav('profile-edit', { isNew: true })}>+ New</Button>} />
      <div className="sub-content">
        {profiles.length === 0 ? <Empty>No custom profiles. Create one to get started.</Empty> :
          profiles.map((p, i) => <div className="profile-card" key={p.id || i}>
            <div className="profile-alias">{p.alias}</div>
            <div className="profile-endpoint">{p.endpoint}</div>
            <div className="profile-meta">
              <span className={`tag ${p.api_key_configured ? 'tag-ok' : 'tag-warn'}`}>{p.api_key_configured ? 'Key ✓' : 'No key'}</span>
              <span className="tag tag-info">{p.model_count || (p.models || []).length || 0} models</span>
            </div>
            <div className="profile-actions" style={{display:'flex',gap:6,marginTop:8}}>
              <Button onClick={() => nav('profile-edit', { isNew: false, profile: p })}>Edit</Button>
              <Button onClick={async () => {
                try {
                  await post('provider-custom', { action: 'test', profile_id: p.id });
                  toast('Models discovered', 'ok');
                  load('providers');
                } catch {}
              }}>Discover</Button>
              <Button tone="danger" onClick={async () => {
                if (!confirm('Delete \"' + p.alias + '\"?')) return;
                try {
                  await post('provider-custom', { action: 'delete', profile_id: p.id });
                  toast('Deleted', 'ok');
                  load('providers');
                } catch {}
              }}>Delete</Button>
            </div>
            {(p.models || []).length > 0 && <div style={{marginTop:12}}>
              <div style={{fontSize:13,fontWeight:600,marginBottom:6}}>Discovered Models</div>
              {p.models.map((m, j) => <div key={j} style={{display:'flex',justifyContent:'space-between',alignItems:'center',padding:'6px 0',borderBottom:'1px solid var(--line)',fontSize:13}}>
                <span style={{fontFamily:'var(--font-mono)',fontSize:12}}>{m.model_id || '-'}</span>
                <div style={{display:'flex',alignItems:'center',gap:8}}>
                  <Status value={m.probe_status} />
                  <Button btn-sm onClick={async () => {
                    try {
                      await post('provider-custom', { action: 'probe', profile_id: p.id, model: m.model_id });
                      toast('Model probed', 'ok');
                      load('providers');
                    } catch {}
                  }}>Probe</Button>
                </div>
              </div>)}
            </div>}
          </div>)}
      </div>
    </div>;
  }

  /* About */
  function AboutPage() {
    return <div className="sub-page">
      <SubHeader title="About Xiao" />
      <div className="sub-content">
        <div style={{textAlign:'center',padding:'30px 0'}}>
          <img src="./xiao-logo.png" alt="Xiao" style={{width:80,height:80,borderRadius:20,marginBottom:12}} />
          <h3 style={{fontSize:20,fontWeight:700}}>Xiao Manager</h3>
          <p style={{color:'var(--muted)',fontSize:13,marginTop:4}}>On-device AI assistant control plane</p>
          <div style={{marginTop:24,display:'flex',justifyContent:'center',gap:8}}>
            <span className="tag tag-ok">v{health?.version || '0.3.1'}</span>
            <span className="tag tag-info">KernelSU</span>
          </div>
        </div>
        <div className="detail-card">
          <DL label="Version">{health?.version || '0.3.1'}</DL>
          <DL label="Architecture">{dash?.runtime?.arch || 'arm64'}</DL>
          <DL label="Gateway">{health?.gateway || 'Ready'}</DL>
          <DL label="Uptime">{formatDuration(health?.uptime_seconds)}</DL>
        </div>
      </div>
    </div>;
  }

  /* ========== SUB-PAGE ROUTER ========== */
  if (sub === 'profile-edit') {
    return <>
      <ProfileEditor
        profile={subArg?.profile}
        isNew={Boolean(subArg?.isNew)}
        busy={busy}
        onBack={() => nav('profiles')}
        onSaved={msg => { toast(msg, 'ok'); load('providers'); nav('profiles'); }}
        onError={err => toast(err, 'bad')}
      />
      <ToastUI />
    </>;
  }

  if (sub) {
    const pages = {
      sessions: SessionsPage,
      'session-detail': SessionDetailPage,
      models: ModelsPage,
      memory: MemoryPage,
      skills: SkillsPage,
      tools: ToolsListPage,
      runs: RunsPage,
      attachments: AttachmentsPage,
      agent: AgentPage,
      runtime: RuntimePage,
      context: ContextPage,
      security: SecurityPage,
      diagnostics: DiagnosticsPage,
      logs: LogsPage,
      telegram: TelegramPage,
      profiles: ProfilesPage,
      about: AboutPage,
    };
    const Page = pages[sub];
    return <>
      {Page ? <Page /> : null}
      {sessionAiDialogTarget && <SessionAiDialog
        session={sessionAiDialogTarget}
        profiles={profiles}
        busy={busy}
        onClose={() => setSessionAiDialogTarget(null)}
        onApply={handleApplySessionAi}
      />}
      <ToastUI />
    </>;
  }

  /* ========== MAIN LAYOUT ========== */
  return <div className={`app-shell ${tab}-screen`}>
    <div className="app-header">
      <div className="header-brand">
        <div className="logo"><img src="./xiao-logo.png" alt="Xiao logo" /></div>
        <h1>Xiao</h1>
        <span className="verified">✓</span>
      </div>
      <div className="header-actions">
        <button
          className="header-btn"
          aria-label="Toggle theme"
          title={`Theme: ${themePreference}`}
          onClick={cycleTheme}
        >
          <AppIcon name={appliedTheme === 'dark' ? 'sun' : 'moon'} size={20} />
        </button>
        <button
          className={`header-btn${refreshing ? ' refreshing' : ''}`}
          aria-label="Refresh"
          title={refreshing ? 'Memperbarui...' : 'Perbarui data'}
          disabled={refreshing}
          onClick={handleManualRefresh}
        >
          <AppIcon name="refresh" size={20} />
        </button>
      </div>
    </div>
    {refreshing && <div className="refresh-bar" />}

    {tab === 'explore' && <ExploreView />}
    {tab === 'tools' && <ToolsView />}
    {tab === 'settings' && <SettingsView />}

    <div className="tab-bar">
      {[['explore','Explore'],['tools','Imagine'],['settings','Settings']].map(([id, label]) =>
        <button
          key={id}
          className={`tab-item${tab === id ? ' active' : ''}`}
          onClick={() => selectTab(id)}
          aria-label={label}
        >
          <AppIcon name={id === 'explore' ? 'spark' : id === 'tools' ? 'search' : 'settings'} size={24} />
          <span>{label}</span>
        </button>
      )}
    </div>
    {sessionAiDialogTarget && <SessionAiDialog
      session={sessionAiDialogTarget}
      profiles={profiles}
      busy={busy}
      onClose={() => setSessionAiDialogTarget(null)}
      onApply={handleApplySessionAi}
    />}
    <ToastUI />
  </div>;
}

export default App;
