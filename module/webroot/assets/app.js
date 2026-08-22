import { exec } from './ksu-bridge.js';

const ACTION = '/data/adb/modules/xiao/action.sh';
const $ = id => document.getElementById(id);
let snapshot = null;
let fetchedModels = [];
let daemonReady = false;

const operationButtons = ['restart', 'refresh', 'fetchModels', 'save'];

function syncControls() {
  const busy = operationButtons.some(id => $(id).getAttribute('aria-busy') === 'true');
  $('restart').disabled = busy;
  $('refresh').disabled = busy;
  $('save').disabled = busy || !daemonReady;
  $('fetchModels').disabled = busy || !daemonReady;
}

function setBusy(id, busy) {
  $(id).setAttribute('aria-busy', String(busy));
  syncControls();
}

const formatDuration = value => {
  const seconds = Number(value || 0);
  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  return [days && `${days}d`, hours && `${hours}h`, minutes && `${minutes}m`, `${seconds % 60}s`]
    .filter(Boolean)
    .join(' ');
};

const encode = value => {
  const bytes = new TextEncoder().encode(value);
  let binary = '';
  bytes.forEach(byte => { binary += String.fromCharCode(byte); });
  return btoa(binary).replaceAll('+', '-').replaceAll('/', '_').replace(/=+$/, '');
};

const wait = milliseconds => new Promise(resolve => setTimeout(resolve, milliseconds));

function addValue(root, labelText, valueText, tone = '') {
  const row = document.createElement('div');
  row.className = 'kv';
  const label = document.createElement('span');
  label.textContent = labelText;
  const value = document.createElement('span');
  value.className = `value ${tone}`.trim();
  value.textContent = String(valueText);
  row.append(label, value);
  root.appendChild(row);
}

async function run(command) {
  const result = await exec(command);
  if (Number(result.errno) !== 0) {
    throw new Error((result.stderr || `exit ${result.errno}`).trim());
  }
  return (result.stdout || '').trim();
}

function showNotice(text, good = true) {
  $('notice').textContent = text;
  $('notice').className = `notice ${good ? 'ok' : 'bad'}`;
}

function showModelNotice(text, tone = '') {
  $('modelNotice').textContent = text;
  $('modelNotice').className = `field-note ${tone || 'muted'}`;
}

function renderModels(models) {
  fetchedModels = [...new Set(models.map(value => String(value).trim()).filter(Boolean))].sort();
  const list = $('customModelList');
  list.innerHTML = '';
  for (const model of fetchedModels) {
    const option = document.createElement('option');
    option.value = model;
    list.appendChild(option);
  }
}

function render() {
  const online = snapshot.daemon.status === 'running';
  $('liveMark').className = `live-mark ${online ? '' : 'offline'}`.trim();
  $('liveMark').querySelector('span').textContent = online ? 'LOKAL' : 'OFFLINE';

  const gateway = $('gateway');
  gateway.innerHTML = '';
  const gatewayState = snapshot.gateway.gateway || 'stopped';
  const gatewayTone = gatewayState === 'running' ? 'ok' : gatewayState === 'degraded' ? 'warn' : 'bad';
  const gatewayLabel = { running: 'Aktif', degraded: 'Terganggu', error: 'Error', stopped: 'Berhenti' }[gatewayState] || gatewayState;
  addValue(gateway, 'Status', gatewayLabel, gatewayTone);
  const telegramState = !snapshot.gateway.telegram_enabled
    ? 'Nonaktif'
    : snapshot.gateway.telegram_polling ? 'Polling' : 'Menunggu';
  addValue(gateway, 'Telegram', telegramState, snapshot.gateway.telegram_polling ? 'ok' : '');

  const daemon = $('daemon');
  daemon.innerHTML = '';
  addValue(daemon, 'Proses', snapshot.daemon.status === 'running' ? 'Berjalan' : 'Berhenti', snapshot.daemon.status === 'running' ? 'ok' : 'bad');
  addValue(daemon, 'Watchdog', snapshot.daemon.watchdog_running ? 'Aktif' : 'Berhenti', snapshot.daemon.watchdog_running ? 'ok' : 'bad');
  addValue(daemon, 'Mulai otomatis', snapshot.daemon.autostart ? 'Aktif' : 'Nonaktif', snapshot.daemon.autostart ? 'ok' : 'bad');
  addValue(daemon, 'Waktu aktif', formatDuration(snapshot.daemon.uptime_seconds));

  $('chatId').value = (snapshot.telegram.allowed_chat_ids || [])[0] || '';
  $('botToken').placeholder = snapshot.telegram.token_configured
    ? 'Sudah tersimpan — kosongkan untuk mempertahankan'
    : 'Masukkan bot token';

  const custom = snapshot.config.custom || {};
  $('customEnabled').checked = Boolean(custom.enabled);
  $('customBase').value = custom.base_url || '';
  $('customProtocol').value = custom.protocol || 'openai_chat_completions';
  $('customModel').value = custom.default_model || '';
  $('customKey').placeholder = custom.api_key_configured
    ? 'Sudah tersimpan — kosongkan untuk mempertahankan'
    : 'Masukkan API key';
  renderModels(custom.models || []);
}

async function refresh() {
  setBusy('refresh', true);
  let lifecycle = null;
  try {
    lifecycle = JSON.parse(await run(`${ACTION} status-json`));
    snapshot = JSON.parse(await run(`${ACTION} snapshot`));
    snapshot.daemon.watchdog_running = Boolean(lifecycle.watchdog.running);
    snapshot.daemon.autostart = Boolean(lifecycle.autostart);
    daemonReady = true;
    render();
    showNotice('Terhubung ke daemon xiao');
  } catch (error) {
    daemonReady = false;
    snapshot = {
      gateway: { gateway: 'stopped', telegram_enabled: false, telegram_polling: false },
      daemon: {
        status: lifecycle?.daemon?.running ? 'running' : 'stopped',
        uptime_seconds: 0,
        watchdog_running: Boolean(lifecycle?.watchdog?.running),
        autostart: Boolean(lifecycle?.autostart)
      },
      telegram: { token_configured: false, allowed_chat_ids: [] },
      config: { custom: {} }
    };
    render();
    console.error('Gagal membaca status xiao:', error);
    showNotice(
      lifecycle
        ? 'Daemon tidak merespons. Coba mulai ulang.'
        : 'Kontrol root tidak tersedia. Buka dari KernelSU Manager.',
      false
    );
  } finally {
    setBusy('refresh', false);
  }
}

$('refresh').onclick = refresh;

$('restart').onclick = async () => {
  setBusy('restart', true);
  try {
    showNotice('Memulai ulang daemon…');
    await run(`${ACTION} restart`);
    await wait(1800);
    await refresh();
  } catch (error) {
    showNotice(`Restart gagal: ${error.message}`, false);
  } finally {
    setBusy('restart', false);
  }
};

$('fetchModels').onclick = async () => {
  setBusy('fetchModels', true);
  try {
    const baseUrl = $('customBase').value.trim();
    if (!baseUrl) throw new Error('Base URL wajib diisi.');
    showModelNotice('Mengambil model…');
    const payload = {
      base_url: baseUrl,
      api_key: $('customKey').value.trim() || null
    };
    const result = JSON.parse(await run(`${ACTION} fetch-models-base64 ${encode(JSON.stringify(payload))}`));
    renderModels(result.models || []);
    if (!fetchedModels.length) throw new Error('Endpoint tidak mengembalikan model.');
    if (!fetchedModels.includes($('customModel').value.trim())) {
      $('customModel').value = fetchedModels[0];
    }
    showModelNotice(`${fetchedModels.length} model tersedia.`, 'ok');
  } catch (error) {
    showModelNotice(`Fetch gagal: ${error.message}`, 'bad');
  } finally {
    setBusy('fetchModels', false);
  }
};

$('save').onclick = async () => {
  setBusy('save', true);
  try {
    const chatId = $('chatId').value.trim();
    const botToken = $('botToken').value.trim();
    if (chatId && !/^-?[1-9]\d*$/.test(chatId)) {
      throw new Error('Chat ID harus berupa angka non-zero.');
    }
    if (chatId && !botToken && !snapshot.telegram.token_configured) {
      throw new Error('Bot token wajib diisi saat Telegram pertama kali diaktifkan.');
    }

    const customEnabled = $('customEnabled').checked;
    const customBase = $('customBase').value.trim();
    const customModel = $('customModel').value.trim();
    if (customEnabled && (!customBase || !customModel)) {
      throw new Error('Base URL dan default model wajib diisi untuk custom provider.');
    }
    const customModels = [...new Set([...fetchedModels, customModel].filter(Boolean))];
    const telegramEnabled = Boolean(chatId && (botToken || snapshot.telegram.token_configured));
    const payload = {
      gateway_enabled: true,
      gateway_auto_restart: true,
      telegram_enabled: telegramEnabled,
      telegram_bot_token: botToken || null,
      allowed_chat_ids: chatId,
      allowed_user_ids: '',
      custom_enabled: customEnabled,
      custom_name: 'Custom',
      custom_base_url: customBase,
      custom_protocol: $('customProtocol').value,
      custom_models: customModels,
      custom_default_model: customModel,
      custom_api_key: $('customKey').value.trim() || null
    };

    showNotice('Memvalidasi dan menyimpan…');
    const result = JSON.parse(await run(`${ACTION} apply-base64 ${encode(JSON.stringify(payload))}`));
    $('botToken').value = '';
    $('customKey').value = '';
    if (result.restart_required) {
      showNotice('Tersimpan. Memulai ulang daemon…');
      await run(`${ACTION} restart`);
      await wait(1800);
    }
    await refresh();
    showNotice('Perubahan tersimpan. Daemon terhubung.');
  } catch (error) {
    showNotice(`Simpan gagal: ${error.message}`, false);
  } finally {
    setBusy('save', false);
  }
};

refresh();
