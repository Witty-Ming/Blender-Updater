/* Blender 更新器 · 前端逻辑
   约定：不使用模板字符串，统一用 createElement 构建节点。 */

const tauri = window.__TAURI__ || {};

function inv(cmd, args) {
  return tauri.core.invoke(cmd, args || {});
}

function $(id) {
  return document.getElementById(id);
}

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined && text !== null) e.textContent = String(text);
  return e;
}

function fmtSize(n) {
  if (!n) return '—';
  const mb = n / 1024 / 1024;
  if (mb >= 1024) return (mb / 1024).toFixed(2) + ' GB';
  return mb.toFixed(1) + ' MB';
}

function shortHash(h) {
  if (!h) return '—';
  return h.length > 10 ? h.slice(0, 10) : h;
}

async function ask(msg, title) {
  try {
    if (tauri.dialog && tauri.dialog.confirm) {
      return await tauri.dialog.confirm(msg, { title: title || '确认', kind: 'warning' });
    }
  } catch (e) { /* 忽略 */ }
  return false;
}

async function pickFolder() {
  try {
    if (tauri.dialog && tauri.dialog.open) {
      const r = await tauri.dialog.open({ directory: true, multiple: false, title: '选择备份位置' });
      return typeof r === 'string' ? r : null;
    }
  } catch (e) { /* 忽略 */ }
  return null;
}

const state = {
  installs: [],
  selected: null,
  channel: 'release',
  results: {},          // 键 path|channel -> CheckResult
  busy: false,
  checking: false,
  snapshots: [],
  settings: null,
  keepCount: 5
};

function ckey(path, channel) {
  return String(path || '').toLowerCase() + '|' + (channel || state.channel);
}

function currentResult() {
  if (!state.selected) return null;
  return state.results[ckey(state.selected, state.channel)] || null;
}

function setStatus(text, kind) {
  $('status-text').textContent = text;
  const dot = $('status-dot');
  dot.className = 'dot' + (kind ? ' ' + kind : '');
}

/* ───────────── 侧栏 ───────────── */

function renderInstalls() {
  const list = $('install-list');
  list.innerHTML = '';
  $('side-count').textContent = state.installs.length
    ? '发现 ' + state.installs.length + ' 个'
    : '未发现';

  if (!state.installs.length) {
    const empty = el('div', 'snap-empty', '还没有找到 Blender 安装');
    list.appendChild(empty);
    return;
  }

  state.installs.forEach(function (ins) {
    const active = ins.path === state.selected;
    const item = el('div', 'install-item' + (active ? ' active' : ''));
    item.appendChild(el('span', 'ii-dot'));

    const main = el('div', 'ii-main');
    main.appendChild(el('div', 'ii-ver', ins.version || ins.version_num || '未知版本'));
    main.appendChild(el('div', 'ii-path', ins.path));
    item.appendChild(main);

    const r = state.results[ckey(ins.path, state.channel)];
    if (r) {
      if (r.status === 'update') {
        item.appendChild(el('span', 'ii-badge update', r.kind === 'rebuild' ? '新构建' : '可更新'));
      } else if (r.status === 'latest') {
        item.appendChild(el('span', 'ii-badge latest', '最新'));
      }
    }

    item.addEventListener('click', function () {
      selectInstall(ins.path);
    });
    list.appendChild(item);
  });
}

function selectInstall(path) {
  if (state.busy) return;
  state.selected = path;
  renderInstalls();
  renderHead();
  renderResult();
  refreshActions();
  const cached = currentResult();
  if (!cached) {
    checkCurrent(false);
  } else {
    setStatus(cached.status === 'update' ? '发现可用更新' : '结果来自缓存');
  }
}

/* ───────────── 头部 ───────────── */

function selectedInstall() {
  for (let i = 0; i < state.installs.length; i++) {
    if (state.installs[i].path === state.selected) return state.installs[i];
  }
  return null;
}

function renderHead() {
  const ins = selectedInstall();
  if (!ins) {
    $('cur-version').textContent = '—';
    $('cur-sub').textContent = '请选择左侧的 Blender 安装';
    $('cur-path').textContent = '—';
    return;
  }
  const big = ins.version_num || ins.version || '—';
  $('cur-version').textContent = big;

  const parts = [];
  const suffix = (ins.version || '').split(' ').slice(1).join(' ');
  if (suffix) parts.push(suffix);
  if (ins.series) parts.push('系列 ' + ins.series);
  if (ins.build_hash) parts.push('构建 ' + shortHash(ins.build_hash));
  if (ins.is_portable) parts.push('便携模式');

  // 目录名与实际版本不一致时明确提示
  const dirName = String(ins.path).split('\\').pop() || '';
  const m = dirName.match(/(\d+\.\d+\.\d+)/);
  if (m && ins.version_num && m[1] !== ins.version_num) {
    parts.push('目录名 ' + m[1]);
  }
  $('cur-sub').textContent = parts.join(' · ') || '版本来自 blender.exe --version';
  $('cur-path').textContent = ins.path;
}

/* ───────────── 更新信息卡 ───────────── */

function kv(label, value, mono) {
  const row = el('div', 'kv');
  row.appendChild(el('span', null, label));
  row.appendChild(el('b', mono ? 'mono' : null, value));
  return row;
}

/* 构建周期与构建日期的显示 */

function cycleLabel(c) {
  return ({ alpha: '内测版', beta: '测试版', candidate: '候选版', release: '正式版' })[c] || '';
}

function btag(version, cycle, date) {
  const parts = [version || ''];
  const c = cycleLabel(cycle);
  if (c) parts.push(c);
  if (date) parts.push(date);
  return parts.filter(Boolean).join(' · ');
}

function renderResult() {
  const slot = $('card-slot');
  slot.innerHTML = '';

  if (!state.selected) return;

  const r = currentResult();
  if (!r) {
    const card = el('div', 'card info-card');
    card.setAttribute('data-kind', 'none');
    const left = el('div', 'info-left');
    left.appendChild(el('span', 'badge', '待检查'));
    left.appendChild(el('div', 'info-title', '还没有检查这个安装'));
    left.appendChild(el('div', 'info-line', '点击右侧「检查更新」获取 ' + (state.channel === 'daily' ? '每日构建' : '稳定版') + ' 通道的最新信息'));
    card.appendChild(left);
    slot.appendChild(card);
    return;
  }

  const kind = r.status === 'error' ? 'error' : (r.status === 'update' ? 'update' : 'latest');
  const card = el('div', 'card info-card');
  card.setAttribute('data-kind', kind);

  const left = el('div', 'info-left');
  if (kind === 'error') {
    left.appendChild(el('span', 'badge error', '检查失败'));
    left.appendChild(el('div', 'info-title', '暂时无法获取版本信息'));
    left.appendChild(el('div', 'info-line', r.error || '未知错误'));
  } else if (kind === 'update') {
    left.appendChild(el('span', 'badge update', r.kind === 'rebuild' ? '同版本新构建' : '可更新'));
    left.appendChild(el('div', 'info-title',
      r.kind === 'rebuild' ? '发现同版本的新构建' : '发现新版本 ' + r.latest_version));
    const line = r.channel === 'daily'
      ? '当前 ' + btag(r.current_version, r.current_cycle, r.current_build_date) +
        ' → 最新 ' + btag(r.latest_version, r.latest_cycle, r.date)
      : (r.kind === 'rebuild'
        ? '当前 ' + shortHash(r.current_hash) + ' → 最新 ' + shortHash(r.latest_hash)
        : '当前 ' + r.current_version + ' → 最新 ' + r.latest_version);
    left.appendChild(el('div', 'info-line', line));
  } else {
    left.appendChild(el('span', 'badge latest', '最新'));
    left.appendChild(el('div', 'info-title', '已是最新版本'));
    const chName = r.channel === 'daily' ? '每日构建' : '稳定版';
    left.appendChild(el('div', 'info-line',
      r.channel === 'daily'
        ? '当前 ' + btag(r.current_version, r.current_cycle, r.current_build_date) + ' 已是该分支的最新构建'
        : '当前 ' + r.current_version + ' 已是 ' + chName + ' 通道的最新版本'));
  }

  const right = el('div', 'info-right');
  const isDaily = r.channel === 'daily';
  if (kind === 'update') {
    right.appendChild(kv('安装包', r.file_name, true));
    right.appendChild(kv('大小', fmtSize(r.size_bytes)));
    if (r.date) right.appendChild(kv(isDaily ? '构建日期' : '发布', r.date));
    if (isDaily && cycleLabel(r.latest_cycle)) {
      right.appendChild(kv('构建类型', cycleLabel(r.latest_cycle)));
    }
  } else if (kind === 'latest') {
    right.appendChild(kv('当前版本', r.current_version));
    right.appendChild(kv('当前构建', shortHash(r.current_hash), true));
    if (r.current_build_date) right.appendChild(kv('构建时间', r.current_build_date));
    right.appendChild(kv('通道', isDaily ? '每日构建' : '稳定版'));
    if (isDaily && r.date) {
      right.appendChild(kv('最新构建', r.latest_version + ' · ' + r.date));
    }
  } else {
    right.appendChild(kv('当前版本', r.current_version));
    right.appendChild(kv('通道', r.channel === 'daily' ? '每日构建' : '稳定版'));
  }
  if (r.source_note) right.appendChild(kv('来源', r.source_note));

  card.appendChild(left);
  card.appendChild(right);
  slot.appendChild(card);

  // 大版本提示（永远不自动覆盖）
  if (r.higher_version && kind !== 'error') {
    const hc = el('div', 'card higher-card');
    hc.appendChild(el('div', 'higher-icon', '!'));
    const body = el('div', 'higher-body');
    body.appendChild(el('div', 'higher-title', '官方已经发布 Blender ' + r.higher_version + '（更高的版本线）'));
    body.appendChild(el('div', 'higher-text', r.higher_note || '跨系列升级不会自动覆盖。'));
    const btn = el('button', 'link-btn', '打开官方下载目录');
    btn.addEventListener('click', function () {
      inv('open_path', { path: r.higher_url });
    });
    body.appendChild(btn);
    hc.appendChild(body);
    slot.appendChild(hc);
  }
}

function refreshActions() {
  const r = currentResult();
  $('btn-update').disabled = !r || r.status !== 'update' || state.busy;
  $('btn-open').disabled = !state.selected;
  $('btn-backup').disabled = !state.selected;
  $('btn-check').disabled = !state.selected || state.checking || state.busy;
}

/* ───────────── 检查更新 ───────────── */

function setChecking(on) {
  state.checking = on;
  $('btn-check').textContent = on ? '检查中…' : '检查更新';
  refreshActions();
}

async function checkCurrent(force) {
  const p = state.selected;
  if (!p || state.checking || state.busy) return;

  setChecking(true);
  setStatus('正在查询 ' + (state.channel === 'daily' ? '每日构建' : '稳定版') + ' 通道…', 'busy');
  try {
    const r = await inv('check_update', { path: p, channel: state.channel, force: !!force });
    state.results[ckey(p, state.channel)] = r;
    renderInstalls();
    renderResult();
    const latestTag = r.channel === 'daily' && r.date
      ? r.latest_version + '（' + r.date + '）'
      : r.latest_version;
    if (r.status === 'update') {
      setStatus(r.kind === 'rebuild' ? '发现同版本的新构建 ' + latestTag : '发现新版本 ' + latestTag, 'ok');
    } else if (r.status === 'latest') {
      setStatus('已是最新版本');
    } else {
      setStatus('检查失败：' + (r.error || '未知错误'), 'err');
    }
  } catch (e) {
    setStatus('检查失败：' + e, 'err');
  } finally {
    setChecking(false);
  }
}

/* ───────────── 执行更新 ───────────── */

function setProgress(p) {
  $('progress-card').hidden = false;
  $('prog-phase').textContent = p.message || '处理中';
  const pct = Math.max(0, Math.min(100, Math.round(p.percent || 0)));
  $('prog-pct').textContent = pct + '%';
  $('prog-fill').style.width = pct + '%';
  $('prog-detail').textContent = p.detail || '';
  setStatus(p.message + (p.detail ? ' · ' + p.detail : ''), 'busy');
}

async function doUpdate() {
  const r = currentResult();
  if (!r || r.status !== 'update' || state.busy) return;
  const ins = selectedInstall();
  const name = ins ? (ins.version || ins.path) : '该安装';

  const ok = await ask(
    '将把 ' + name + ' 更新到 ' + r.latest_version + '。\n\n' +
    '· 更新会覆盖程序文件，目录路径保持不变\n' +
    '· portable/config 会先备份并原样保留\n' +
    '· 更新过程中请不要启动这个 Blender\n\n继续？',
    '确认更新'
  );
  if (!ok) return;

  state.busy = true;
  refreshActions();
  $('progress-card').hidden = false;
  $('prog-phase').textContent = '准备更新';
  $('prog-pct').textContent = '0%';
  $('prog-fill').style.width = '0%';
  $('prog-detail').textContent = r.file_name || '';

  try {
    const msg = await inv('start_update', { path: state.selected, channel: state.channel });
    state.busy = false;
    setStatus(msg, 'ok');
    $('prog-phase').textContent = '更新完成';
    $('prog-pct').textContent = '100%';
    $('prog-fill').style.width = '100%';
    $('prog-detail').textContent = msg;
    await afterUpdate();
  } catch (e) {
    state.busy = false;
    const text = String(e);
    if (text.indexOf('已取消') >= 0) {
      setStatus('已取消更新');
      $('progress-card').hidden = true;
    } else {
      setStatus('更新失败：' + text, 'err');
      $('prog-phase').textContent = '更新失败';
      $('prog-pct').textContent = '';
      $('prog-detail').textContent = text;
    }
  }
  refreshActions();
}

async function afterUpdate() {
  state.results = {};
  await rescan(false);
  if (state.selected) {
    renderHead();
    renderResult();
    await checkCurrent(true);
  }
}

/* ───────────── 扫描 ───────────── */

async function rescan(keepSelection) {
  try {
    const list = await inv('scan_installs');
    state.installs = list || [];
    if (!keepSelection) {
      const still = state.installs.some(function (i) { return i.path === state.selected; });
      if (!still) {
        state.selected = state.installs.length ? state.installs[0].path : null;
      }
    }
    if (!state.selected && state.installs.length) {
      state.selected = state.installs[0].path;
    }
    renderInstalls();
    renderHead();
    renderResult();
    refreshActions();
  } catch (e) {
    setStatus('扫描失败：' + e, 'err');
  }
}

/* ───────────── 备份面板 ───────────── */

function reasonLabel(r) {
  if (r === 'manual') return '手动备份';
  if (r === 'before_install') return '更新前';
  if (r === 'before_restore') return '恢复前';
  return r;
}

function renderSnapshots() {
  const list = $('snap-list');
  list.innerHTML = '';
  $('snap-count').textContent = '共 ' + state.snapshots.length + ' 份备份';
  $('backup-path').textContent = state.selected
    ? state.selected + '\\portable\\config'
    : '—';

  if (!state.snapshots.length) {
    list.appendChild(el('div', 'snap-empty',
      '还没有备份。\n点击「立即备份」保存当前的 portable/config。'));
    return;
  }

  state.snapshots.forEach(function (s) {
    const item = el('div', 'snap-item');
    item.appendChild(el('div', 'snap-colorbar ' + s.reason));

    const main = el('div', 'snap-main');
    const top = el('div', 'snap-top');
    const dt = String(s.created_at || '').split(' ');
    top.appendChild(el('span', 'snap-date', dt[0] || s.created_at));
    top.appendChild(el('span', 'snap-time', dt[1] || ''));
    top.appendChild(el('span', 'snap-reason ' + s.reason, reasonLabel(s.reason)));
    if (s.locked) top.appendChild(el('span', 'snap-lock', '永久保留'));
    main.appendChild(top);

    const meta = [];
    if (s.blender_version) meta.push('Blender ' + s.blender_version);
    meta.push(s.file_count + ' 个文件');
    meta.push(fmtSize(s.bytes));
    meta.push(s.id);
    main.appendChild(el('div', 'snap-meta', meta.join(' · ')));
    item.appendChild(main);

    const acts = el('div', 'snap-actions');
    const btnRestore = el('button', 'ghost-btn', '恢复');
    btnRestore.addEventListener('click', function () { restoreSnapshot(s); });
    const btnDel = el('button', 'ghost-btn', '删除');
    btnDel.addEventListener('click', function () { deleteSnapshot(s); });
    acts.appendChild(btnRestore);
    acts.appendChild(btnDel);
    item.appendChild(acts);

    list.appendChild(item);
  });
}

async function openBackup() {
  if (!state.selected) return;
  $('backup-mask').hidden = false;
  $('keep-select').value = String(state.keepCount || 5);
  await loadSnapshots();
}

async function loadSnapshots() {
  try {
    const list = await inv('list_snapshots', { path: state.selected });
    state.snapshots = list || [];
    renderSnapshots();
  } catch (e) {
    $('snap-list').innerHTML = '';
    $('snap-list').appendChild(el('div', 'snap-empty', '读取备份失败：' + e));
  }
}

async function makeSnapshot() {
  try {
    const list = await inv('make_snapshot', { path: state.selected });
    state.snapshots = list || [];
    renderSnapshots();
    setStatus('已保存配置备份', 'ok');
  } catch (e) {
    setStatus('备份失败：' + e, 'err');
  }
}

async function restoreSnapshot(s) {
  const ok = await ask(
    '恢复 ' + s.created_at + ' 的配置备份？\n\n当前 portable/config 会先自动存一份「恢复前」备份。',
    '确认恢复'
  );
  if (!ok) return;
  try {
    const list = await inv('restore_snapshot', { path: state.selected, id: s.id });
    state.snapshots = list || [];
    renderSnapshots();
    setStatus('已恢复到 ' + s.created_at + ' 的配置', 'ok');
  } catch (e) {
    setStatus('恢复失败：' + e, 'err');
  }
}

async function deleteSnapshot(s) {
  const ok = await ask('删除 ' + s.created_at + ' 的配置备份？此操作不可撤销。', '删除备份');
  if (!ok) return;
  try {
    const list = await inv('delete_snapshot', { path: state.selected, id: s.id });
    state.snapshots = list || [];
    renderSnapshots();
    setStatus('已删除备份', 'ok');
  } catch (e) {
    setStatus('删除失败：' + e, 'err');
  }
}

/* ───────────── 设置 ───────────── */

function renderSettings(s) {
  if (!s) return;
  state.settings = s;
  state.keepCount = s.keep_count;
  $('set-snap-dir').value = s.snapshot_dir || '';
  $('set-snap-dir').dataset.original = s.snapshot_dir || '';
  $('set-keep').value = s.keep_count;
  $('set-auto-backup').checked = !!s.auto_backup;
  $('set-prefer-mirror').checked = !!s.prefer_mirror;
  $('set-version').textContent = 'v' + (s.app_version || '');
  $('tb-version').textContent = 'v' + (s.app_version || '');
  $('set-author').textContent = s.author || 'WittyMing';
  $('set-data-dir').textContent = s.data_dir || '';
  $('set-snap-note').textContent = s.snapshot_dir_custom ? '当前为自定义位置' : '当前为默认位置';
  $('set-snap-note').className = 'set-note';
}

async function loadSettings() {
  try {
    const s = await inv('get_settings');
    renderSettings(s);
  } catch (e) {
    $('set-status').textContent = '读取设置失败：' + e;
    $('set-status').className = 'set-status err';
  }
}

async function browseSnapshotDir() {
  const dir = await pickFolder();
  if (!dir) return;
  $('set-snap-dir').value = dir;
  const original = $('set-snap-dir').dataset.original || '';
  if (dir !== original) {
    $('set-snap-note').textContent = '保存后会把已有快照迁移到新位置';
    $('set-snap-note').className = 'set-note warn';
  }
}

function resetSnapshotDir() {
  const def = state.settings ? state.settings.default_snapshot_dir : '';
  $('set-snap-dir').value = def;
  $('set-snap-note').textContent = '保存后会把已有快照迁移回默认位置';
  $('set-snap-note').className = 'set-note warn';
}

async function saveSettings() {
  if (!state.settings) return;
  const keep = parseInt($('set-keep').value, 10);
  if (!keep || keep < 1 || keep > 50) {
    $('set-status').textContent = '保留份数需要是 1 到 50 之间的整数';
    $('set-status').className = 'set-status err';
    return;
  }
  const next = Object.assign({}, state.settings, {
    snapshot_dir: $('set-snap-dir').value,
    keep_count: keep,
    auto_backup: $('set-auto-backup').checked,
    prefer_mirror: $('set-prefer-mirror').checked
  });

  $('btn-save-settings').disabled = true;
  $('set-status').textContent = '正在保存…';
  $('set-status').className = 'set-status';
  try {
    const rep = await inv('save_settings', { next: next });
    let msg = '设置已保存';
    if (rep && rep.moved > 0) msg += '，已迁移 ' + rep.moved + ' 份快照';
    if (rep && rep.failed > 0) msg += '（' + rep.failed + ' 份因重名未迁移）';
    $('set-status').textContent = msg;
    $('set-status').className = 'set-status ok';
    state.keepCount = keep;
    await loadSettings();
  } catch (e) {
    $('set-status').textContent = '保存失败：' + e;
    $('set-status').className = 'set-status err';
  }
  $('btn-save-settings').disabled = false;
}

function switchView(which) {
  const main = which === 'settings';
  $('view-main').hidden = main;
  $('view-settings').hidden = !main;
  if (main) loadSettings();
}

/* ───────────── 事件绑定 ───────────── */

function bind() {
  // 窗口控制
  $('btn-min').addEventListener('click', function () { inv('win_minimize'); });
  $('btn-max').addEventListener('click', function () { inv('win_toggle_maximize'); });
  $('btn-close').addEventListener('click', function () { inv('win_close'); });
  $('btn-settings').addEventListener('click', function () { switchView('settings'); });
  $('btn-set-back').addEventListener('click', function () { switchView('main'); });

  const drag = document.querySelector('.tb-drag');
  if (drag) {
    drag.addEventListener('dblclick', function () { inv('win_toggle_maximize'); });
  }

  // 侧栏
  $('btn-rescan').addEventListener('click', function () {
    setStatus('正在扫描…', 'busy');
    rescan(true).then(function () { setStatus('扫描完成'); });
  });

  $('btn-add').addEventListener('click', async function () {
    const dir = await pickFolder();
    if (!dir) return;
    try {
      const list = await inv('add_install', { path: dir });
      state.installs = list || [];
      state.selected = dir.toLowerCase();
      for (let i = 0; i < state.installs.length; i++) {
        if (state.installs[i].path.toLowerCase() === dir.toLowerCase()) {
          state.selected = state.installs[i].path;
        }
      }
      renderInstalls();
      renderHead();
      renderResult();
      refreshActions();
      checkCurrent(false);
    } catch (e) {
      setStatus('添加失败：' + e, 'err');
    }
  });

  // 通道
  const segs = document.querySelectorAll('#channel-seg .seg');
  segs.forEach(function (btn) {
    btn.addEventListener('click', function () {
      if (state.busy) return;
      const ch = btn.dataset.channel;
      if (ch === state.channel) return;
      state.channel = ch;
      segs.forEach(function (b) {
        b.classList.toggle('active', b.dataset.channel === ch);
      });
      renderInstalls();
      renderResult();
      refreshActions();
      const cached = currentResult();
      if (cached) {
        setStatus('结果来自缓存');
      } else {
        checkCurrent(false);
      }
    });
  });

  $('btn-check').addEventListener('click', function () { checkCurrent(true); });
  $('btn-update').addEventListener('click', doUpdate);
  $('btn-backup').addEventListener('click', openBackup);
  $('btn-open').addEventListener('click', function () {
    if (state.selected) inv('open_path', { path: state.selected });
  });

  // 备份浮层
  $('backup-close').addEventListener('click', function () { $('backup-mask').hidden = true; });
  $('backup-mask').addEventListener('click', function (e) {
    if (e.target === $('backup-mask')) $('backup-mask').hidden = true;
  });
  $('btn-make-snapshot').addEventListener('click', makeSnapshot);
  $('btn-open-snapdir').addEventListener('click', function () {
    if (state.selected) inv('open_snapshot_dir', { path: state.selected });
  });
  $('keep-select').addEventListener('change', async function () {
    const n = parseInt($('keep-select').value, 10);
    try {
      await inv('set_keep_count', { count: n });
      state.keepCount = n;
      await loadSnapshots();
      setStatus('自动备份保留 ' + n + ' 份');
    } catch (e) {
      setStatus('设置保留份数失败：' + e, 'err');
    }
  });

  // 设置页
  $('btn-snap-browse').addEventListener('click', browseSnapshotDir);
  $('btn-snap-reset').addEventListener('click', resetSnapshotDir);
  $('set-keep-minus').addEventListener('click', function () {
    const v = Math.max(1, (parseInt($('set-keep').value, 10) || 5) - 1);
    $('set-keep').value = v;
  });
  $('set-keep-plus').addEventListener('click', function () {
    const v = Math.min(50, (parseInt($('set-keep').value, 10) || 5) + 1);
    $('set-keep').value = v;
  });
  $('btn-save-settings').addEventListener('click', saveSettings);
  $('set-author').addEventListener('click', function () {
    if (state.settings && state.settings.homepage) {
      inv('open_path', { path: state.settings.homepage });
    }
  });
  $('set-data-dir').addEventListener('click', function () {
    if (state.settings && state.settings.data_dir) {
      inv('open_path', { path: state.settings.data_dir });
    }
  });

  // 后端事件
  if (tauri.event && tauri.event.listen) {
    tauri.event.listen('update-progress', function (e) {
      setProgress(e.payload || {});
    });
    tauri.event.listen('update-done', function (e) {
      const p = e.payload || {};
      if (p.ok) {
        $('progress-card').hidden = false;
        $('prog-phase').textContent = '更新完成';
        $('prog-pct').textContent = '100%';
        $('prog-fill').style.width = '100%';
        setStatus(p.message || '更新完成', 'ok');
      } else {
        setStatus(p.message || '更新失败', 'err');
      }
    });
    tauri.event.listen('snapshot-changed', function () {
      if (!$('backup-mask').hidden && state.selected) loadSnapshots();
    });
  }
}

/* ───────────── 启动 ───────────── */

async function boot() {
  bind();
  await loadSettings();
  await rescan(false);
  if (state.selected) {
    setStatus('就绪');
    checkCurrent(false);
  } else {
    setStatus('没有发现 Blender 安装，可以手动添加目录');
  }
}

boot();
