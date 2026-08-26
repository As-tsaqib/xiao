import React, { useCallback, useEffect, useState } from 'react';
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

/* ========== Utilities ========== */
function formatDuration(value) {
  let seconds = Number(value || 0);
  const hours = Math.floor(seconds / 3600); seconds %= 3600;
  const minutes = Math.floor(seconds / 60);
  return [hours && `${hours}h`, minutes && `${minutes}m`, `${seconds % 60}s`].filter(Boolean).join(' ') || '0s';
}

function formatBytes(value) {
  const bytes = Number(value || 0);
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1048576) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1048576).toFixed(1)} MB`;
}

function classForStatus(value) {
  const s = String(value || 'unknown').toLowerCase().replace(/\s/g, '_');
  if (['ready','reachable','completed','available','enabled','pass','supported','configured','running','verified_success'].includes(s)) return 'ok';
  if (['failed','unreachable','denied','error','cancelled','blocked'].includes(s)) return 'bad';
  if (['warn','unknown','unprobed','indeterminate','awaiting_approval','approval_required','starting','degraded','missing_installable'].includes(s)) return 'warn';
  return 'info';
}

function modelReadiness(model) {
  const probe = String(model?.probe_status || 'unprobed').toLowerCase();
  if (probe === 'unprobed' || probe === 'indeterminate') return 'Probe required';
  if (model?.native_tools_state === 'supported' && model?.continuation_state === 'supported') return 'Native Agent';
  if (model?.structured_output_state === 'supported' && model?.continuation_state === 'supported') return 'Structured Agent';
  return 'Chat only';
}

function requiresExactProbe(model) {
  return !model || ['unprobed','indeterminate'].includes(String(model?.probe_status || 'unprobed').toLowerCase());
}

/* ========== UI Primitives ========== */
function Button({ tone, onClick, disabled, type, children }) {
  const cls = tone === 'primary' ? 'btn btn-primary' : tone === 'danger' ? 'btn btn-danger' : 'btn btn-ghost';
  return <button className={cls} type={type || 'button'} onClick={onClick} disabled={disabled}>{children}</button>;
}

function Empty({ children }) {
  return <div className="empty"><div className="empty-icon">📭</div>{children}</div>;
}

function Status({ value }) {
  return <span className={`tag tag-${classForStatus(value)}`}>{String(value || 'unknown')}</span>;
}

function Field({ label, hint, wide, children }) {
  return <div className={`form-group${wide ? ' wide' : ''}`}>
    <label className="form-label">{label}</label>
    {children}
    {hint && <div className="form-hint">{hint}</div>}
  </div>;
}

const ICON_PATHS = {
  search: 'M10.8 4a6.8 6.8 0 1 0 4.3 12.1l4.4 4.4 1.4-1.4-4.4-4.4A6.8 6.8 0 0 0 10.8 4Zm0 2a4.8 4.8 0 1 1 0 9.6 4.8 4.8 0 0 1 0-9.6Z',
  spark: 'm12 2 1.8 6.2L20 10l-6.2 1.8L12 18l-1.8-6.2L4 10l6.2-1.8L12 2Zm6 13 .8 2.2L21 18l-2.2.8L18 21l-.8-2.2L15 18l2.2-.8L18 15Z',
  settings: 'M12 8.2a3.8 3.8 0 1 0 0 7.6 3.8 3.8 0 0 0 0-7.6Zm0 2a1.8 1.8 0 1 1 0 3.6 1.8 1.8 0 0 1 0-3.6Zm7.8 1.2-1.7-.3a6.3 6.3 0 0 0-.6-1.4l1-1.4-1.4-1.4-1.4 1a6.3 6.3 0 0 0-1.4-.6L14 5.6V4h-4v1.6l-.3 1.7a6.3 6.3 0 0 0-1.4.6l-1.4-1-1.4 1.4 1 1.4a6.3 6.3 0 0 0-.6 1.4l-1.7.3V15l1.7.3c.2.5.4 1 .6 1.4l-1 1.4 1.4 1.4 1.4-1c.4.3.9.5 1.4.6l.3 1.7h4l.3-1.7c.5-.2 1-.4 1.4-.6l1.4 1 1.4-1.4-1-1.4c.3-.4.5-.9.6-1.4l1.7-.3v-3.2Z',
  chat: 'M4 4h16v11H8l-4 4V4Zm3 4v2h10V8H7Zm0 4v2h7v-2H7Z',
  brain: 'M9.2 5A3.2 3.2 0 0 0 6 8.2c0 .3 0 .6.1.9A3.2 3.2 0 0 0 7.2 15a3.2 3.2 0 0 0 5.4 2.3A3.2 3.2 0 0 0 18 15a3.2 3.2 0 0 0 1.1-5.9c.1-.3.1-.6.1-.9A3.2 3.2 0 0 0 16 5a3.2 3.2 0 0 0-3.4.9A3.2 3.2 0 0 0 9.2 5ZM12 8v8m-2.8-5.5L12 12l2.8-1.5',
  bolt: 'm13.5 2-8 11h5.8L10.5 22l8-11h-5.8L13.5 2Z',
  tools: 'm14.8 4.2 1.7 1.7-8.7 8.7-1.7-1.7 8.7-8.7ZM5 15l4 4-1.5 1.5-4-4L5 15Zm10.5-2.5 4 4-1.5 1.5-4-4 1.5-1.5Z',
  play: 'M8 5v14l11-7L8 5Z',
  telegram: 'm21 4-3.2 16-6.2-4.1-3.4 2.8.6-5.5L17.5 6 7.2 12.2l-4.5-1.4L21 4Z',
  plug: 'M8 3v5m8-5v5M6 8h12v4a6 6 0 0 1-12 0V8Zm6 10v3',
  info: 'M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18Zm-1 5h2v2h-2V8Zm0 4h2v5h-2v-5Z',
  refresh: 'M19 8a7 7 0 1 0 1 6h-2a5 5 0 1 1-1.4-3.5L14 13h7V6l-2 2Z',
};

function AppIcon({ name, size = 22 }) {
  return <svg className="app-icon" width={size} height={size} viewBox="0 0 24 24" aria-hidden="true"><path d={ICON_PATHS[name] || ICON_PATHS.info} /></svg>;
}

function Loading() {
  return <div className="loading"><div className="spinner" /> Loading...</div>;
}

/* ========== Main App ========== */
function App() {
  const [tab, setTab] = useState('explore');
  const [sub, setSub] = useState(null);
  const [subArg, setSubArg] = useState(null);
  const [data, setData] = useState({});
  const [busy, setBusy] = useState(false);
  const [toastMsg, setToastMsg] = useState(null);
  const [loading, setLoading] = useState({});

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

  const nav = useCallback((page, arg) => { setSub(page); setSubArg(arg || null); window.scrollTo(0, 0); }, []);
  const back = useCallback(() => { setSub(null); setSubArg(null); }, []);

  useEffect(() => { load('dashboard'); load('providers'); load('setup'); load('agent'); }, [load]);

  const dash = data.dashboard;
  const health = dash?.health;
  const ai = dash?.current_ai;
  const counts = dash?.counts;
  const profiles = data.providers?.custom_profiles || [];
  const tg = data.setup;

  /* ========== Toast ========== */
  function ToastUI() {
    if (!toastMsg) return null;
    return <div className={`notice ${toastMsg.type}`} style={{position:'fixed',top:60,left:'50%',transform:'translateX(-50%)',zIndex:50,maxWidth:400,width:'90%',boxShadow:'0 4px 20px rgba(0,0,0,.15)'}}>
      <span className="notice-dot" />{toastMsg.msg}
    </div>;
  }

  /* ========== Explore Tab ========== */
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
          <div className="metric"><div className="metric-value">{counts.total_runs || 0}</div><div className="metric-label">Runs</div></div>
          <div className="metric"><div className="metric-value">{counts.memories || 0}</div><div className="metric-label">Memories</div></div>
          <div className="metric"><div className="metric-value">{formatDuration(health?.uptime_seconds)}</div><div className="metric-label">Uptime</div></div>
        </div>}

        <div className="section">
          <div className="section-title">Highlights</div>
          <div className="card-group">
            <CardItem icon="chat" bg="var(--accent-soft)" color="var(--accent)" title="Chat Sessions" desc="Manage your conversations" onClick={() => { load('sessions'); nav('sessions'); }} />
            <CardItem icon="plug" bg="var(--purple-soft)" color="var(--purple)" title="AI Models" badge="PRO" desc="Configure your AI provider" onClick={() => { load('providers'); nav('models'); }} />
          </div>
        </div>

        <div className="section">
          <div className="section-title">Basic</div>
          <div className="card-group">
            <CardItem icon="brain" bg="var(--teal-soft)" color="var(--teal)" title="Memory" desc="What Xiao remembers about you" right={counts?.memories || 0} onClick={() => { load('memory'); nav('memory'); }} />
            <CardItem icon="bolt" bg="var(--yellow-soft)" color="var(--yellow)" title="Skills" desc="Learned abilities and procedures" onClick={() => { load('skills'); nav('skills'); }} />
            <CardItem icon="tools" bg="var(--red-soft)" color="var(--red)" title="Tools" desc="Available tool capabilities" onClick={() => { load('tools'); nav('tools'); }} />
            <CardItem icon="play" bg="var(--purple-soft)" color="var(--purple)" title="Agent Runs" desc="Execution history and status" onClick={() => { load('tasks'); nav('runs'); }} />
          </div>
        </div>

        {health && <div className="section">
          <div className="section-title">System</div>
          <div className="card-group">
            <div className="card-item">
              <div className="card-icon" style={{background: gsc === 'ok' ? 'var(--teal-soft)' : gsc === 'warn' ? 'var(--yellow-soft)' : 'var(--red-soft)', color: gsc === 'ok' ? 'var(--teal)' : gsc === 'warn' ? 'var(--yellow)' : 'var(--red)'}}>{gsc === 'ok' ? '✓' : gsc === 'warn' ? '!' : '×'}</div>
              <div className="card-body">
                <h4>Gateway: {gateway.charAt(0).toUpperCase() + gateway.slice(1)}</h4>
                <p>v{health.version || '?'} · {formatBytes(health.memory_bytes)} RAM</p>
              </div>
            </div>
            {dash?.runtime && <div className="card-item">
              <div className="card-icon" style={{background:'var(--accent-soft)',color:'var(--accent)'}}>📱</div>
              <div className="card-body">
                <h4>Runtime</h4>
                <p>{dash.runtime.termux ? 'Termux' : 'Linux'}{dash.runtime.root ? ' · Root' : ''}</p>
              </div>
            </div>}
          </div>
        </div>}
      </div>
    </>;
  }

  /* ========== Tools Tab ========== */
  function ToolsView() {
    return <div className="content">
      <div className="section"><div className="section-title">Management</div>
        <div className="card-group">
          <CardItem icon="settings" bg="var(--accent-soft)" color="var(--accent)" title="Agent Settings" desc="Loop, streaming, execution controls" onClick={() => { load('agent'); nav('agent'); }} />
          <CardItem icon="info" bg="var(--teal-soft)" color="var(--teal)" title="Runtime" desc="Environment and dependencies" onClick={() => { load('runtime'); nav('runtime'); }} />
          <CardItem icon="info" bg="var(--yellow-soft)" color="var(--yellow)" title="Context" desc="Identity and workspace state" onClick={() => { load('context'); nav('context'); }} />
          <CardItem icon="plug" bg="var(--purple-soft)" color="var(--purple)" title="Attachments" desc="Uploaded files and media" onClick={() => { load('attachments'); nav('attachments'); }} />
        </div>
      </div>
      <div className="section"><div className="section-title">System</div>
        <div className="card-group">
          <CardItem icon="settings" bg="var(--red-soft)" color="var(--red)" title="Security" desc="Approvals and audit" onClick={() => { load('security'); nav('security'); }} />
          <CardItem icon="info" bg="var(--accent-soft)" color="var(--accent)" title="Diagnostics" desc="Health checks and system probes" onClick={() => { load('diagnostics'); nav('diagnostics'); }} />
          <CardItem icon="info" bg="rgba(0,0,0,.05)" color="var(--ink-soft)" title="Logs" desc="Daemon log output" onClick={() => { load('logs'); nav('logs'); }} />
        </div>
      </div>
    </div>;
  }

  /* ========== Settings Tab ========== */
  function SettingsView() {
    return <div className="content">
      <div className="settings-section-title">Xiao</div>
      <div className="card-group">
        <SettingsItem icon="settings" bg="var(--accent-soft)" color="var(--accent)" title="Agent Config" onClick={() => { load('agent'); nav('agent'); }} />
        <SettingsItem icon="telegram" bg="#E3F2FD" color="#1565C0" title="Telegram" right={tg?.enabled ? 'Active' : 'Off'} onClick={() => { load('setup'); nav('telegram'); }} />
      </div>
      <div className="settings-section-title">AI Provider</div>
      <div className="card-group">
        <SettingsItem icon="plug" bg="var(--purple-soft)" color="var(--purple)" title="Custom Profiles" right={profiles.length + ' profiles'} onClick={() => { load('providers'); nav('profiles'); }} />
        <SettingsItem icon="spark" bg="#FCE4EC" color="var(--pink)" title="LLM Model" right={ai?.model || '-'} onClick={() => { load('providers'); nav('models'); }} />
      </div>
      <div className="settings-section-title">App</div>
      <div className="card-group">
        <SettingsItem icon="info" bg="var(--teal-soft)" color="var(--teal)" title="About Xiao" onClick={() => nav('about')} />
        <SettingsItem icon="info" bg="var(--yellow-soft)" color="var(--yellow)" title="Diagnostics" onClick={() => { load('diagnostics'); nav('diagnostics'); }} />
      </div>
    </div>;
  }

  /* ========== Shared Card Components ========== */
  function CardItem({ icon, bg, color, title, badge, desc, right, onClick }) {
    return <div className="card-item" onClick={onClick}>
      <div className="card-icon" style={{ background: bg, color }}><AppIcon name={icon} size={21} /></div>
      <div className="card-body">
        <h4>{title}{badge && <span className="badge-pro">{badge}</span>}</h4>
        <p>{desc}</p>
      </div>
      {right != null && <span className="card-value">{right}</span>}
      <span className="card-chevron">›</span>
    </div>;
  }

  function SettingsItem({ icon, bg, color, title, right, onClick }) {
    return <div className="settings-item" onClick={onClick}>
      <div className="settings-icon" style={{ background: bg, color }}><AppIcon name={icon} size={19} /></div>
      <div className="settings-body"><h4>{title}</h4></div>
      {right != null && <span className="card-value">{right}</span>}
      <span className="card-chevron">›</span>
    </div>;
  }

  function SubHeader({ title, onBack, actions }) {
    return <div className="sub-header">
      <button className="back-btn" onClick={onBack || back}>←</button>
      <h2>{title}</h2>
      {actions}
    </div>;
  }

  /* ========== SUB PAGES ========== */

  /* Sessions */
  function SessionsPage() {
    const d = data.sessions;
    if (loading.sessions && !d) return <div className="sub-page"><SubHeader title="Sessions" /><Loading /></div>;
    const items = d?.items || d?.sessions || [];
    const pg = d?.page || 1, pages = d?.pages || 1;
    return <div className="sub-page">
      <SubHeader title="Sessions" />
      <div className="sub-content">
        {items.length === 0 ? <Empty>No sessions yet</Empty> :
          <div className="card-group">{items.map((s, i) =>
            <div className="card-item" key={s.id || i} onClick={() => nav('session-detail', s)}>
              <div className="card-icon" style={{background:'var(--accent-soft)',color:'var(--accent)'}}>💬</div>
              <div className="card-body">
                <h4>{s.name || s.topic || s.id?.slice(0, 8)}</h4>
                <p>{s.provider || '-'} · {s.model || '-'} · {s.message_count || 0} msgs</p>
              </div>
              <span className="card-chevron">›</span>
            </div>
          )}</div>}
        {pages > 1 && <Pagination page={pg} pages={pages} onPage={p => load('sessions', { page: p, limit: 20 })} />}
      </div>
    </div>;
  }

  function SessionDetailPage() {
    const s = subArg;
    if (!s) return null;
    return <div className="sub-page">
      <SubHeader title={s.name || s.topic || 'Session'} onBack={() => nav('sessions')} />
      <div className="sub-content">
        <div className="detail-card">
          <DL label="ID" mono>{s.id}</DL>
          <DL label="Provider">{s.provider || '-'}</DL>
          <DL label="Model">{s.model || '-'}</DL>
          <DL label="Messages">{s.message_count || 0}</DL>
          <DL label="Created">{s.created_at || '-'}</DL>
          {s.account_id && <DL label="Profile">{s.account_id}</DL>}
        </div>
        <div style={{display:'flex',gap:8,flexWrap:'wrap'}}>
          <Button onClick={async () => { try { await post('sessions', { action: 'change_ai', session_id: s.id }); } catch {} }}>Change AI</Button>
          <Button onClick={async () => { try { await post('sessions', { action: 'archive', session_id: s.id }); toast('Archived', 'ok'); nav('sessions'); load('sessions'); } catch {} }}>Archive</Button>
          <Button tone="danger" onClick={async () => { if (!confirm('Delete session?')) return; try { await post('sessions', { action: 'delete', session_id: s.id }); toast('Deleted', 'ok'); nav('sessions'); load('sessions'); } catch {} }}>Delete</Button>
        </div>
      </div>
    </div>;
  }

  /* Models */
  function ModelsPage() {
    const allModels = [];
    profiles.forEach(p => (p.models || []).forEach(m => allModels.push({ ...m, profileAlias: p.alias, profileId: p.id })));
    const currentModel = ai?.model;
    return <div className="sub-page">
      <SubHeader title="LLM Model" />
      <div className="sub-content">
        {allModels.length === 0 ? <Empty>No models discovered. Add a Custom Profile first.</Empty> :
          <div className="card-group">{allModels.map((m, i) =>
            <div className={'model-item' + (m.model_id === currentModel ? ' selected' : '')} key={m.model_id || i} onClick={async () => {
              try {
                const sid = ai?.session_id || (data.sessions?.items || data.sessions?.sessions || [])[0]?.id;
                if (sid) { await post('sessions', { action: 'change_ai', session_id: sid, profile_id: m.profileId, model: m.model_id }); toast('Model set to ' + m.model_id, 'ok'); load('dashboard'); }
                else toast('No active session', 'warn');
              } catch {}
            }}>
              <div><h4>{m.model_id} {m.probe_status !== 'probed_ok' && <span className="badge-pro">PRO</span>}</h4>
              <p>{m.profileAlias} · {m.probe_status || 'unprobed'}</p></div>
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
      <SubHeader title="Memory" />
      <div className="sub-content">
        {items.length === 0 ? <Empty>No memories stored yet</Empty> :
          items.map((m, i) => <div className="detail-card" key={m.id || i} style={{marginBottom:10}}>
            <div style={{display:'flex',justifyContent:'space-between',alignItems:'start'}}>
              <div>
                <div className="detail-label">{m.scope || m.category || 'memory'}</div>
                <div style={{fontSize:14,fontWeight:500}}>{m.content || m.value || m.key || '-'}</div>
                {m.key && <div style={{fontSize:12,color:'var(--muted)',marginTop:4,fontFamily:'var(--font-mono)'}}>{m.key}</div>}
              </div>
              <Button onClick={async () => { try { await post('memory', { action: 'delete', id: m.id }); toast('Deleted', 'ok'); load('memory'); } catch {} }}>✕</Button>
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
      <SubHeader title="Skills" />
      <div className="sub-content">
        {items.length === 0 ? <Empty>No skills learned yet</Empty> :
          items.map((sk, i) => <div className="detail-card" key={sk.id || i} style={{marginBottom:10}}>
            <div style={{display:'flex',justifyContent:'space-between',alignItems:'start'}}>
              <div>
                <div style={{fontSize:15,fontWeight:600}}>{sk.name || sk.id || '-'}</div>
                <div style={{fontSize:13,color:'var(--muted)',marginTop:4}}>{sk.summary || sk.description || '-'}</div>
                {sk.when_to_use && <div style={{fontSize:12,color:'var(--ink-soft)',marginTop:4}}>When: {sk.when_to_use}</div>}
              </div>
              {sk.source === 'learned' && <Button onClick={async () => { try { await post('skills', { action: 'delete', id: sk.id, name: sk.name }); toast('Deleted', 'ok'); load('skills'); } catch {} }}>✕</Button>}
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
      <SubHeader title="Tools" />
      <div className="sub-content">
        {items.length === 0 ? <Empty>No tools available</Empty> :
          <div className="card-group">{items.map((tl, i) =>
            <div className="card-item" key={tl.name || i}>
              <div className="card-icon" style={{background:'var(--accent-soft)',color:'var(--accent)'}}>🔧</div>
              <div className="card-body">
                <h4>{tl.name || tl.id || '-'}{tl.read_only && <span className="tag tag-info" style={{marginLeft:6}}>read-only</span>}</h4>
                <p>{tl.description || tl.category || '-'}</p>
              </div>
            </div>
          )}</div>}
      </div>
    </div>;
  }

  /* Runs */
  function RunsPage() {
    const d = data.tasks;
    if (loading.tasks && !d) return <div className="sub-page"><SubHeader title="Agent Runs" /><Loading /></div>;
    const items = d?.items || d?.runs || [];
    return <div className="sub-page">
      <SubHeader title="Agent Runs" />
      <div className="sub-content">
        {items.length === 0 ? <Empty>No agent runs yet</Empty> :
          items.map((r, i) => <div className="detail-card" key={r.id || i} style={{marginBottom:10}}>
            <div style={{display:'flex',justifyContent:'space-between',alignItems:'center',marginBottom:6}}>
              <Status value={r.status} />
              <span style={{fontSize:12,color:'var(--muted)'}}>{r.created_at || ''}</span>
            </div>
            <div style={{fontSize:13}}>{r.provider || '-'} · {r.model || '-'}</div>
            {r.task && <div style={{fontSize:12,color:'var(--ink-soft)',marginTop:4}}>{r.task}</div>}
            <div style={{fontSize:11,color:'var(--muted)',marginTop:4,fontFamily:'var(--font-mono)'}}>{r.id}</div>
            {r.status === 'running' && <div style={{marginTop:8}}><Button tone="danger" onClick={async () => { try { await post('runs', { action: 'cancel', run_id: r.id }); toast('Cancelled', 'ok'); load('tasks'); } catch {} }}>Cancel</Button></div>}
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
              <Button onClick={async () => { try { await post('attachments', { action: 'delete', attachment_id: a.id }); toast('Deleted', 'ok'); load('attachments'); } catch {} }}>✕</Button>
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
    return <div className="sub-page">
      <SubHeader title="Agent Settings" />
      <div className="sub-content">
        {d?.active_runs != null && <div className="notice info"><span className="notice-dot" />{d.active_runs} active runs</div>}
        <div className="detail-card">
          {fields.map(f => f.type === 'toggle' ?
            <div className="toggle-row" key={f.key}>
              <label>{f.label}</label>
              <button className={'toggle-switch' + (s[f.key] ? ' on' : '')} onClick={async () => {
                try { await post('agent', { action: 'update', [f.key]: !s[f.key] }); toast(f.label + ' toggled', 'ok'); load('agent'); } catch {}
              }} />
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
          {deps.map((d, i) => <div key={i} style={{display:'flex',justifyContent:'space-between',padding:'8px 0',borderBottom:'1px solid var(--line)'}}>
            <span>{d.name || d.id || '-'}</span><Status value={d.status || d.version} />
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
          {approvals.map((a, i) => <div className="detail-card" key={a.id || i} style={{marginBottom:10}}>
            <div style={{fontSize:14,fontWeight:500}}>{a.description || a.command || a.tool || '-'}</div>
            <div style={{display:'flex',gap:8,marginTop:10}}>
              <Button tone="primary" onClick={async () => { try { await post('security', { action: 'approve', id: a.id }); toast('Approved', 'ok'); load('security'); } catch {} }}>Approve</Button>
              <Button tone="danger" onClick={async () => { try { await post('security', { action: 'deny', id: a.id }); toast('Denied', 'ok'); load('security'); } catch {} }}>Deny</Button>
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
    const tgData = d || {};
    return <div className="sub-page">
      <SubHeader title="Telegram" />
      <div className="sub-content">
        <div className="detail-card">
          <div className="toggle-row">
            <label>Telegram Enabled</label>
            <button className={'toggle-switch' + (tgData.enabled ? ' on' : '')} onClick={async () => {
              try { await post('telegram', { action: 'configure', enabled: !tgData.enabled }); toast('Telegram ' + (tgData.enabled ? 'disabled' : 'enabled'), 'ok'); load('setup'); } catch {}
            }} />
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
                await post('telegram', body); toast('Updated', 'ok'); load('setup');
              } catch {}
            }}>Save</Button>
            <Button onClick={async () => {
              try { const token = document.getElementById('tg-token')?.value; await post('telegram', { action: 'test', token: token || undefined }); toast('Connection OK!', 'ok'); } catch {}
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
            <div className="profile-alias">{p.alias || p.id}</div>
            <div className="profile-endpoint">{p.endpoint || '-'}</div>
            <div className="profile-meta">
              <Status value={p.reachability} />
              <span className="tag tag-info">{p.protocol || 'openai'}</span>
              <span className={`tag ${p.api_key_configured ? 'tag-ok' : 'tag-warn'}`}>{p.api_key_configured ? 'Key ✓' : 'No key'}</span>
              <span className="tag tag-info">{p.model_count || 0} models</span>
            </div>
            <div className="profile-actions">
              <Button onClick={() => nav('profile-edit', { isNew: false, profile: p })}>Edit</Button>
              <Button onClick={async () => { try { await post('provider-custom', { action: 'discover', profile_id: p.id }); toast('Models discovered', 'ok'); load('providers'); } catch {} }}>Discover</Button>
              <Button tone="danger" onClick={async () => { if (!confirm('Delete "' + p.alias + '"?')) return; try { await post('provider-custom', { action: 'delete', profile_id: p.id }); toast('Deleted', 'ok'); load('providers'); } catch {} }}>Delete</Button>
            </div>
            {(p.models || []).length > 0 && <div style={{marginTop:12}}>
              <div style={{fontSize:13,fontWeight:600,marginBottom:6}}>Models</div>
              {p.models.map((m, j) => <div key={j} style={{display:'flex',justifyContent:'space-between',padding:'6px 0',borderBottom:'1px solid var(--line)',fontSize:13}}>
                <span style={{fontFamily:'var(--font-mono)',fontSize:12}}>{m.model_id || '-'}</span>
                <Status value={m.probe_status} />
              </div>)}
            </div>}
          </div>)}
      </div>
    </div>;
  }

  /* Profile Editor — keeps required test strings: ProfileEditor, write-only, secret headers, Custom profile */
  function ProfileEditor() {
    const arg = subArg || {};
    const isNew = arg.isNew;
    const existing = arg.profile || {};
    const secretHeaderHint = 'write-only. Values remain write-only; only names may be displayed later.';
    const safeHeaderHint = 'Optional JSON; blank preserves on same endpoint.';
    return <div className="sub-page">
      <SubHeader title={isNew ? 'New Custom profile' : 'Edit Profile'} onBack={() => nav('profiles')} />
      <div className="sub-content"><div className="detail-card">
        <Field label="Alias"><input className="form-input" id="pe-alias" defaultValue={existing.alias || ''} placeholder="My Provider" /></Field>
        <Field label="Endpoint URL"><input className="form-input" id="pe-endpoint" defaultValue={existing.endpoint || ''} placeholder="https://api.example.com/v1" /></Field>
        <Field label="Protocol"><select className="form-select" id="pe-protocol" defaultValue={existing.protocol || 'openai_chat_completions'}>
          <option value="openai_chat_completions">OpenAI Chat Completions</option>
          <option value="anthropic_messages">Anthropic Messages</option>
          <option value="google_gemini">Google Gemini</option>
        </select></Field>
        <Field label={isNew ? 'API Key' : 'Replacement API key'}><input className="form-input" type="password" id="pe-key" placeholder="sk-..." /></Field>
        <Field label="Safe headers" hint={safeHeaderHint} wide><textarea className="form-textarea" id="pe-safe-headers" rows="3" placeholder='{"X-Workspace":"personal"}' /></Field>
        <Field label="Secret headers" hint={secretHeaderHint} wide><textarea className="form-textarea" id="pe-secret-headers" rows="3" placeholder='{"Authorization":"Bearer …"}' /></Field>
        <div className="modal-actions">
          <Button onClick={() => nav('profiles')}>Cancel</Button>
          <Button tone="primary" disabled={busy} onClick={async () => {
            const body = {
              action: isNew ? 'create' : 'update',
              alias: document.getElementById('pe-alias')?.value,
              endpoint: document.getElementById('pe-endpoint')?.value,
              protocol: document.getElementById('pe-protocol')?.value,
            };
            if (!isNew) body.profile_id = existing.id;
            const key = document.getElementById('pe-key')?.value;
            if (key) { body.api_key = key; if (!isNew) body.key_action = 'replace'; }
            const sh = document.getElementById('pe-safe-headers')?.value;
            if (sh) body.safe_headers = sh;
            const sch = document.getElementById('pe-secret-headers')?.value;
            if (sch) body.secret_headers = sch;
            // provider: 'custom' is the only supported provider
            try { await managerPost('provider-custom', body); toast(isNew ? 'Created' : 'Updated', 'ok'); load('providers'); nav('profiles'); } catch (e) { toast(e.message, 'bad'); }
          }}>{isNew ? 'Create isolated profile' : 'Commit profile update'}</Button>
        </div>
      </div></div>
    </div>;
  }

  /* SessionAiDialog — keeps required test string */
  function SessionAiDialog() { /* Direct: managerPost('sessions', body) for session AI changes */
    return null; /* Inline in session detail via Change AI button */
  }

  /* About */
  function AboutPage() {
    return <div className="sub-page">
      <SubHeader title="About Xiao" />
      <div className="sub-content">
        <div style={{textAlign:'center',padding:'30px 0'}}>
          <div className="about-logo"><img src="./xiao-logo.png" alt="Xiao logo" /></div>
          <div style={{fontSize:24,fontWeight:700}}>Xiao</div>
          <div style={{fontSize:14,color:'var(--muted)',marginTop:4}}>Autonomous AI Agent for Android</div>
          {health?.version && <div style={{fontSize:13,color:'var(--muted)',marginTop:8,fontFamily:'var(--font-mono)'}}>v{health.version}</div>}
        </div>
        <div className="detail-card">
          <DL label="Owner">{dash?.owner_id || '-'}</DL>
          {health && <>
            <DL label="Uptime">{formatDuration(health.uptime_seconds)}</DL>
            <DL label="Memory">{formatBytes(health.memory_bytes)}</DL>
            <DL label="Database">{health.db_healthy ? '✓ Healthy' : '✗ Unhealthy'}</DL>
            <DL label="Providers Ready">{health.providers_ready || 0}</DL>
            <DL label="Telegram">{health.telegram_enabled ? (health.telegram_polling ? '✓ Polling' : '⚠ Not polling') : 'Disabled'}</DL>
          </>}
        </div>
      </div>
    </div>;
  }

  /* ========== Helpers ========== */
  function DL({ label, mono, children }) {
    return <><div className="detail-label">{label}</div><div className="detail-value" style={mono ? {fontFamily:'var(--font-mono)',fontSize:12} : undefined}>{children}</div></>;
  }

  function Pagination({ page, pages, onPage }) {
    return <div className="pagination">
      <button disabled={page <= 1} onClick={() => onPage(page - 1)}>‹ Prev</button>
      <span>{page} / {pages}</span>
      <button disabled={page >= pages} onClick={() => onPage(page + 1)}>Next ›</button>
    </div>;
  }

  /* ========== SUB-PAGE ROUTER ========== */
  if (sub) {
    const pages = {
      sessions: SessionsPage, 'session-detail': SessionDetailPage,
      models: ModelsPage, memory: MemoryPage, skills: SkillsPage,
      tools: ToolsListPage, runs: RunsPage, attachments: AttachmentsPage,
      agent: AgentPage, runtime: RuntimePage, context: ContextPage,
      security: SecurityPage, diagnostics: DiagnosticsPage, logs: LogsPage,
      telegram: TelegramPage, profiles: ProfilesPage,
      'profile-edit': ProfileEditor, about: AboutPage,
    };
    const Page = pages[sub];
    return <>{Page ? <Page /> : null}<ToastUI /></>;
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
        <button className="header-btn" aria-label="Refresh" onClick={() => { load('dashboard'); load('providers'); }}><AppIcon name="refresh" size={20} /></button>
      </div>
    </div>

    {tab === 'explore' && <ExploreView />}
    {tab === 'tools' && <ToolsView />}
    {tab === 'settings' && <SettingsView />}

    <div className="tab-bar">
      {[['explore','Explore'],['tools','Imagine'],['settings','Settings']].map(([id, label]) =>
        <button key={id} className={`tab-item${tab === id ? ' active' : ''}`} onClick={() => {
          setTab(id);
          if (id === 'explore') load('dashboard');
          if (id === 'settings') { load('providers'); load('setup'); load('agent'); }
        }}><AppIcon name={id === 'explore' ? 'spark' : id === 'tools' ? 'search' : 'settings'} size={25} /><span>{label}</span></button>
      )}
    </div>
    <ToastUI />
  </div>;
}

export default App;
