// Settings: bindings and video, set once and overridable per core and per
// game (ADR 0028). Every layer holds only what it changes; this page draws
// what each button resolves to and which layer it came from. A binding has
// two halves, keyboard-and-mouse (`keys`) and gamepad (`pad`), which inherit
// apart. Every change is
// sent to Rust at once, which validates it, saves it and sends back what it
// kept. It applies from the next round dealt.
//
// Uses `$`, `invoke` and `esc` from index.html's own script.

(() => {
  let S = null;        // the settings, as Rust last returned them
  let meta = null;     // games, cores, the RetroPad, guest keys, built-ins
  let tab = 'controls';
  let scope = 'global';      // 'global' | 'core:<id>' | 'game:<id>'
  let vscope = 'global';     // 'global' | '<core id>'
  let capturing = false;
  let held = [];
  let timer = null;
  let settingsPath = null;

  // Roughly what each console draws, for the size preview only. The real
  // size comes from the core when a round loads.
  const PICTURE = { nes: [256, 240], snes: [256, 224], dos: [320, 200] };

  // ---- labels ---------------------------------------------------------------
  const KEY_LABEL = { lctrl: 'L Ctrl', rctrl: 'R Ctrl', lshift: 'L Shift', rshift: 'R Shift', lalt: 'L Alt',
    ralt: 'R Alt', space: 'Space', enter: 'Enter', escape: 'Esc', tab: 'Tab', backspace: 'Backspace',
    up: '↑', down: '↓', left: '←', right: '→', comma: ',', period: '.', minus: '-', equals: '=',
    semicolon: ';', apostrophe: "'", slash: '/', backslash: '\\', lbracket: '[', rbracket: ']' };
  const PAD_LABEL = { south: 'South', east: 'East', west: 'West', north: 'North', lb: 'LB', rb: 'RB',
    lt: 'LT', rt: 'RT', l3: 'L3', r3: 'R3', start: 'Start', select: 'Select', guide: 'Guide',
    dup: 'D-pad ↑', ddown: 'D-pad ↓', dleft: 'D-pad ←', dright: 'D-pad →' };
  const ARROW = { up: '↑', down: '↓', left: '←', right: '→' };
  const keyName = k => KEY_LABEL[k] || k.toUpperCase();
  function label(input) {
    const [kind, rest] = input.split(':');
    if (kind === 'key') return keyName(rest);
    if (kind === 'mouse') return 'Mouse ' + rest.toUpperCase();
    if (kind === 'pad') return '🎮 ' + (PAD_LABEL[rest] || rest);
    if (kind === 'stick') { const [s, d] = rest.split('-'); return '🎮 ' + (s === 'left' ? 'L' : 'R') + '-stick ' + ARROW[d]; }
    return input;
  }
  const chordLabel = c => c.map(label).join(' + ');
  const isPad = i => i.startsWith('pad:') || i.startsWith('stick:');

  // ---- layers -----------------------------------------------------------------
  const gameById = id => meta.games.find(g => g.id === id);
  const coreGames = core => meta.games.filter(g => g.core === core);
  const cores = () => [...new Set(meta.games.map(g => g.core))].sort();
  const coreName = c => c.startsWith('missing:') ? c.slice(8) + ' (core missing)' : c;

  // The games a scope speaks for, for its layout, buttons and controller.
  function scopeGame(sc) {
    if (sc.startsWith('game:')) return gameById(sc.slice(5));
    if (sc.startsWith('core:')) return coreGames(sc.slice(5))[0];
    return null;
  }
  function layerOf(sc, create) {
    let map, id;
    if (sc.startsWith('core:')) { map = 'cores'; id = sc.slice(5); }
    else if (sc.startsWith('game:')) { map = 'games'; id = sc.slice(5); }
    else return null;
    if (!S[map][id] && create) S[map][id] = {};
    return S[map][id] || null;
  }
  const famOf = i => isPad(i) ? 'pad' : 'keys';
  // The layers under a scope for one half, nearest first, ending at the
  // built-in mapping. "All games" shows the four-button layout; a two-button
  // console keeps its own for any button not set there.
  function chain(sc, fam) {
    const g = scopeGame(sc);
    const builtin = g ? g.builtin : meta.games.find(x => x.layout === 'snes')?.builtin || { keys: {}, pad: {} };
    const out = [];
    if (sc.startsWith('game:')) out.push(['game', S.games[g.id]?.[fam]]);
    if (sc.startsWith('game:') || sc.startsWith('core:')) out.push(['core', S.cores[g.core]?.[fam]]);
    out.push(['global', S.input[fam]]);
    out.push(['default', builtin[fam]]);
    return out;
  }
  function resolve(sc, b, fam) {
    for (const [from, binds] of chain(sc, fam)) {
      if (binds && b in binds) return { list: binds[b], from };
    }
    return { list: [], from: 'default' };
  }
  // Both halves, as play reads them: a keyboard game's keys pass through.
  function resolveBoth(sc, b) {
    const g = scopeGame(sc);
    const keys = g?.controller === 'keyboard' ? [] : resolve(sc, b, 'keys').list;
    return [...keys, ...resolve(sc, b, 'pad').list];
  }
  function setOwn(sc, b, fam, list) {
    if (sc === 'global') { S.input[fam][b] = list; return; }
    const l = layerOf(sc, true);
    l[fam] = l[fam] || {};
    l[fam][b] = list;
  }
  function resetOwn(sc, b) {
    for (const fam of ['keys', 'pad']) {
      if (sc === 'global') delete S.input[fam][b];
      else { const l = layerOf(sc); if (l?.[fam]) delete l[fam][b]; }
    }
  }
  function padKeyFor(g, b) {
    const gl = S.games[g.id]?.pad_keys, cl = S.cores[g.core]?.pad_keys;
    if (gl && b in gl) return { key: gl[b], from: 'game' };
    if (cl && b in cl) return { key: cl[b], from: 'core' };
    if (b in g.pad_keys) return { key: g.pad_keys[b], from: 'manifest' };
    return { key: '', from: 'default' };
  }

  // ---- saving -------------------------------------------------------------------
  async function save() {
    try {
      S = await invoke('put_settings', { settings: S });
      $('name').value = S.name;
      $('s-status').textContent = 'Saved. Applies from the next round.';
    } catch (e) {
      $('s-status').textContent = 'Not saved: ' + e;
    }
    render();
  }

  // ---- capture --------------------------------------------------------------------
  function swallow(e) { if (capturing) { e.preventDefault(); e.stopPropagation(); } }
  window.addEventListener('keydown', swallow, true);
  window.addEventListener('keyup', swallow, true);
  window.addEventListener('beforeunload', () => { if (capturing) invoke('cancel_capture'); });

  async function capture(chord) {
    if (capturing) return null;
    capturing = true;
    if (document.activeElement) document.activeElement.blur();
    $('s-capture-text').textContent = chord
      ? 'Hold the combination, then let go. Esc cancels.'
      : 'Press a key, a mouse button, or a gamepad button or stick. Esc cancels.';
    $('s-capture').classList.remove('hidden');
    try {
      return await invoke('capture_input', { chord });
    } catch (e) {
      $('s-status').textContent = 'Capture failed: ' + e;
      return null;
    } finally {
      capturing = false;
      $('s-capture').classList.add('hidden');
    }
  }

  // Whatever was pressed goes in its own half: a pad button pressed into a
  // keyboard slot is added to the pad's.
  async function bindSlot(b, fam, slot) {
    const got = await capture(false);
    if (!got) return;
    const input = got[0];
    const into = famOf(input);
    if (into === 'keys' && scopeGame(scope)?.controller === 'keyboard') {
      $('s-status').textContent = 'This game takes the keyboard as it is; bind a gamepad input here, or remap a key below.';
      return;
    }
    const list = [...resolve(scope, b, into).list];
    const at = list.indexOf(input);
    if (at >= 0) list.splice(at, 1);
    if (into !== fam || slot >= list.length) list.push(input); else list[slot] = input;
    setOwn(scope, b, into, list.slice(0, meta.max_slots));
    // A stick bound to a button here goes through it, not its own key.
    const own = layerOf(scope)?.stick_keys;
    if (own && input in own) delete own[input];
    save();
  }
  function clearSlot(b, fam, slot) {
    const list = [...resolve(scope, b, fam).list];
    list.splice(slot, 1);
    setOwn(scope, b, fam, list);
    save();
  }

  // ---- the pad ----------------------------------------------------------------------
  // A generic modern pad, laid out by RetroPad name: the SNES diamond every
  // modern pad shares, shoulders and triggers, a d-pad, two sticks. It is the
  // same drawing whatever pad is plugged in. A stick's cap is L3 or R3; its
  // four directions are not RetroPad buttons but inputs, so they are drawn
  // round the cap with what each presses.
  const ORDER = ['up', 'down', 'left', 'right', 'b', 'a', 'y', 'x', 'l', 'r', 'l2', 'r2', 'l3', 'r3', 'select', 'start'];
  const buttonsOf = g => g ? ORDER.filter(b => g.buttons.includes(b)) : ORDER;
  const PAD_SHAPES = {
    l2: ['rect', 110, 14, 80, 28], r2: ['rect', 410, 14, 80, 28],
    l: ['rect', 95, 48, 110, 20], r: ['rect', 395, 48, 110, 20],
    up: ['rect', 208, 184, 28, 28], down: ['rect', 208, 240, 28, 28],
    left: ['rect', 180, 212, 28, 28], right: ['rect', 236, 212, 28, 28],
    x: ['circle', 450, 112, 18], a: ['circle', 488, 150, 18], b: ['circle', 450, 188, 18], y: ['circle', 412, 150, 18],
    select: ['rect', 242, 142, 40, 16], start: ['rect', 318, 142, 40, 16],
    l3: ['circle', 150, 150, 17], r3: ['circle', 380, 226, 17],
  };
  const STICKS = { left: [150, 150], right: [380, 226] };
  const DIRS = ['up', 'down', 'left', 'right'];
  // What a button is called on the pad: the key it presses, for a keyboard game.
  const padText = (g, b) => g?.controller === 'keyboard' ? (keyName(padKeyFor(g, b).key || '')) : (ARROW[b] || b.toUpperCase());
  // The buttons a stick direction presses here, from every button's pad half.
  const stickTargets = (g, inp) => buttonsOf(g).filter(b => resolve(scope, b, 'pad').list.includes(inp));
  // A keyboard game's stick direction may send a key of its own instead
  // (ADR 0031): the game's, then the core's. Null when it goes through a button.
  function stickKeyFor(g, inp) {
    if (g?.controller !== 'keyboard') return null;
    const gl = S.games[g.id]?.stick_keys, cl = S.cores[g.core]?.stick_keys;
    if (gl && inp in gl) return { key: gl[inp], from: 'game' };
    if (cl && inp in cl) return { key: cl[inp], from: 'core' };
    return null;
  }
  // What a stick direction ends up pressing, as the pad draws it.
  function stickText(g, inp) {
    const own = stickKeyFor(g, inp);
    if (own) return own.key ? keyName(own.key) : '';
    const on = stickTargets(g, inp);
    return on.length ? padText(g, on[0]) + (on.length > 1 ? '+' : '') : '';
  }
  function stickTitle(g, inp, name) {
    const own = stickKeyFor(g, inp);
    if (own) return name + (own.key ? ' sends ' + keyName(own.key) : ' sends nothing');
    const on = stickTargets(g, inp);
    return name + (on.length ? ' presses ' + on.map(b => b.toUpperCase()).join(' and ') : ' presses nothing');
  }
  function wedge(cx, cy, d) {
    const mid = { up: -90, right: 0, down: 90, left: 180 }[d];
    const pt = (r, a) => (cx + r * Math.cos(a * Math.PI / 180)).toFixed(1) + ' ' + (cy + r * Math.sin(a * Math.PI / 180)).toFixed(1);
    return 'M' + pt(42, mid - 45) + ' A42 42 0 0 1 ' + pt(42, mid + 45) + ' L' + pt(19, mid + 45) + ' A19 19 0 0 0 ' + pt(19, mid - 45) + 'Z';
  }
  function padSvg(g) {
    const has = g ? new Set(g.buttons) : null;
    let h = '<svg viewBox="0 0 600 330" class="pad" role="img" aria-label="Gamepad">' +
      '<path d="M150 72 Q300 55 450 72 Q548 82 570 185 Q590 300 528 312 Q482 320 452 278 L148 278 Q118 320 72 312 Q10 300 30 185 Q52 82 150 72 Z" class="body"/>' +
      '<rect class="hub" x="208" y="212" width="28" height="28"/><circle class="hub" cx="300" cy="104" r="13"/>';
    for (const [s, [cx, cy]] of Object.entries(STICKS)) {
      h += '<circle class="well" cx="' + cx + '" cy="' + cy + '" r="44"/>';
      for (const d of DIRS) {
        const inp = 'stick:' + s + '-' + d;
        const name = (s === 'left' ? 'Left' : 'Right') + ' stick ' + ARROW[d];
        h += '<path class="dir' + (inp === selIn ? ' sel' : '') + '" data-in="' + inp + '" d="' + wedge(cx, cy, d) + '"><title>' +
             esc(stickTitle(g, inp, name)) + '</title></path>';
        const a = { up: -90, right: 0, down: 90, left: 180 }[d] * Math.PI / 180;
        h += '<text x="' + (cx + 31 * Math.cos(a)).toFixed(1) + '" y="' + (cy + 31 * Math.sin(a) + 3.5).toFixed(1) + '" class="lbl tiny">' +
             esc(stickText(g, inp)) + '</text>';
      }
    }
    for (const [b, s] of Object.entries(PAD_SHAPES)) {
      const dim = has && !has.has(b) ? ' dim' : '';
      const on = (b === sel ? ' sel' : '');
      const attrs = 'class="btn' + dim + on + '" data-b="' + b + '"';
      if (s[0] === 'rect') h += '<rect ' + attrs + ' x="' + s[1] + '" y="' + s[2] + '" width="' + s[3] + '" height="' + s[4] + '" rx="6"/>';
      else h += '<circle ' + attrs + ' cx="' + s[1] + '" cy="' + s[2] + '" r="' + s[3] + '"/>';
      const cx = s[0] === 'rect' ? s[1] + s[3] / 2 : s[1];
      const cy = s[0] === 'rect' ? s[2] + s[4] / 2 : s[2];
      const small = s[0] === 'rect' ? s[4] < 20 : s[3] < 18;
      h += '<text x="' + cx + '" y="' + (cy + (small ? 4 : 5)) + '" data-b="' + b + '" class="lbl' + dim + (small ? ' sm' : '') + '">' + esc(padText(g, b)) + '</text>';
    }
    return h + '</svg>';
  }
  let sel = null;      // the button picked on the pad
  let selIn = null;    // or the stick direction

  // Each stick direction, and the one button it presses. Choosing one moves
  // that input there, out of whichever button had it. In a keyboard game it
  // is the key it sends instead, which no button's key can then move.
  function stickTable(g) {
    const keyboard = g?.controller === 'keyboard';
    const bs = buttonsOf(g);
    let h = '<table class="binds sticks"><tr><th></th>' + DIRS.map(d => '<th>' + ARROW[d] + '</th>').join('') + '</tr>';
    for (const s of ['left', 'right']) {
      h += '<tr><td class="bname">' + (s === 'left' ? 'Left' : 'Right') + ' stick</td>';
      for (const d of DIRS) {
        const inp = 'stick:' + s + '-' + d;
        const on = stickTargets(g, inp);
        let o = '';
        if (keyboard) {
          const own = stickKeyFor(g, inp);
          const via = on.length ? 'via ' + on.map(b => b.toUpperCase() + ' (' + (keyName(padKeyFor(g, b).key || '') || 'nothing') + ')').join(' + ') : 'via no button';
          o += '<option value="via"' + (own ? '' : ' selected') + '>— ' + esc(via) + '</option>';
          o += '<option value=""' + (own && !own.key ? ' selected' : '') + '>— nothing</option>';
          for (const k of meta.guest_keys) o += '<option value="' + k + '"' + (own?.key === k ? ' selected' : '') + '>' + esc(keyName(k)) + '</option>';
        } else {
          o += '<option value=""' + (on.length ? '' : ' selected') + '>— nothing</option>';
          if (on.length > 1) o += '<option value="*" selected disabled>' + esc(on.map(b => b.toUpperCase()).join(' + ')) + '</option>';
          for (const b of bs) o += '<option value="' + b + '"' + (on.length === 1 && on[0] === b ? ' selected' : '') + '>' + esc(b.toUpperCase()) + '</option>';
        }
        h += '<td><select class="stickto' + (inp === selIn ? ' sel' : '') + '" data-in="' + inp + '">' + o + '</select></td>';
      }
      h += '</tr>';
    }
    return h + '</table><div class="note">A stick is read as four directions: pushed past the deadzone, a direction ' +
      (keyboard ? 'sends the key chosen here, or presses its button like any other input. ' : 'presses its button like any other input. ') +
      'No game is sent a stick\'s position.</div>';
  }
  // A keyboard game: the key a stick direction sends, or "via" to send it
  // through its button again. A key of its own takes it out of every
  // button's list here, as play does.
  function bindStickKey(inp, v) {
    const l = layerOf(scope, true);
    l.stick_keys = l.stick_keys || {};
    if (v === 'via') delete l.stick_keys[inp];
    else {
      l.stick_keys[inp] = v;
      for (const b of buttonsOf(scopeGame(scope))) {
        const list = resolve(scope, b, 'pad').list;
        if (list.includes(inp)) setOwn(scope, b, 'pad', list.filter(i => i !== inp));
      }
    }
    save();
  }
  function bindStick(inp, to) {
    const bs = buttonsOf(scopeGame(scope));
    if (to && !resolve(scope, to, 'pad').list.includes(inp) && resolve(scope, to, 'pad').list.length >= meta.max_slots) {
      $('s-status').textContent = to.toUpperCase() + ' already has ' + meta.max_slots + ' gamepad inputs; unbind one first.';
      render();
      return;
    }
    for (const b of bs) {
      const list = resolve(scope, b, 'pad').list;
      const bound = list.includes(inp);
      if (b === to && !bound) setOwn(scope, b, 'pad', [...list, inp]);
      else if (b !== to && bound) setOwn(scope, b, 'pad', list.filter(i => i !== inp));
    }
    save();
  }

  // ---- render: controls ------------------------------------------------------------
  function scopeList() {
    let h = '<div class="scope' + (scope === 'global' ? ' on' : '') + '" data-scope="global">All games</div>';
    for (const c of cores()) {
      const gs = coreGames(c);
      h += '<div class="scope' + (scope === 'core:' + c ? ' on' : '') + '" data-scope="core:' + esc(c) + '">' +
           esc(coreName(c)) + ' <span class="sub">' + esc([...new Set(gs.map(g => g.console.toUpperCase()))].join(', ')) + '</span></div>';
      for (const g of gs.sort((a, b) => a.title.localeCompare(b.title))) {
        h += '<div class="scope game' + (scope === 'game:' + g.id ? ' on' : '') + '" data-scope="game:' + esc(g.id) + '">' + esc(g.title) + '</div>';
      }
    }
    return h;
  }

  const FROM = { game: 'this game', core: 'this core', global: 'all games', default: 'default', manifest: 'game default' };
  const here = sc => sc.startsWith('game:') ? 'game' : sc.startsWith('core:') ? 'core' : 'global';

  function bindTable(g) {
    const keyboard = g?.controller === 'keyboard';
    const fams = keyboard ? ['pad'] : ['keys', 'pad'];
    let h = '<table class="binds"><tr><th>Button</th>' +
      fams.map(f => '<th colspan="' + meta.max_slots + '" class="fam">' + (f === 'keys' ? 'Keyboard &amp; mouse' :
        'Gamepad' + (keyboard ? ' <span class="sub">(the keyboard and mouse pass straight through)</span>' : '')) + '</th>').join('') +
      (keyboard ? '<th>Presses</th>' : '') + '<th></th></tr>';
    for (const b of buttonsOf(g)) {
      let mineAny = false, froms = [];
      h += '<tr data-row="' + b + '" class="' + (b === sel ? 'sel' : '') + '"><td class="bname">' + b.toUpperCase() + '</td>';
      for (const fam of fams) {
        const r = resolve(scope, b, fam);
        const mine = r.from === here(scope);
        mineAny ||= mine;
        if (!mine) froms.push(FROM[r.from]);
        for (let i = 0; i < meta.max_slots; i++) {
          const inp = r.list[i];
          const lit = inp && held.includes(inp) ? ' lit' : '';
          const at = ' data-b="' + b + '" data-f="' + fam + '" data-i="' + i + '"';
          if (inp) h += '<td' + (i === 0 ? ' class="fam"' : '') + '><span class="slot' + (mine ? '' : ' inh') + lit + '"' + at +
                        ' title="' + (mine ? 'set here' : 'from ' + FROM[r.from]) + '">' + esc(label(inp)) +
                        '<span class="x"' + at + ' title="Unbind">×</span></span></td>';
          else if (i === r.list.length) h += '<td' + (i === 0 ? ' class="fam"' : '') + '><span class="slot add"' + at + '>+</span></td>';
          else h += '<td></td>';
        }
      }
      if (keyboard) {
        const pk = padKeyFor(g, b);
        h += '<td><select class="padkey" data-b="' + b + '"><option value="">— nothing</option>' +
             meta.guest_keys.map(k => '<option value="' + k + '"' + (k === pk.key ? ' selected' : '') + '>' + esc(keyName(k)) + '</option>').join('') +
             '</select> <span class="from">' + FROM[pk.from] + '</span></td>';
      }
      h += '<td class="from">' + (mineAny ? '<a href="#" class="reset" data-b="' + b + '">reset</a>' :
        'from ' + [...new Set(froms)].join(' / ')) + '</td></tr>';
    }
    return h + '</table>';
  }

  function remapTable(g) {
    const l = S.games[g.id]?.key_remap || {};
    let h = '<h4>Keyboard remap</h4><div class="note">A key not listed types itself. "Nothing" silences it.</div><table class="remap">';
    const opts = (v, none) => (none ? '<option value="">— nothing</option>' : '') +
      meta.guest_keys.map(k => '<option value="' + k + '"' + (k === v ? ' selected' : '') + '>' + esc(keyName(k)) + '</option>').join('');
    for (const [from, to] of Object.entries(l)) {
      h += '<tr><td>' + esc(keyName(from)) + '</td><td>types</td><td><select class="remap-to" data-from="' + esc(from) + '">' + opts(to, true) +
           '</select></td><td><a href="#" class="remap-del" data-from="' + esc(from) + '">remove</a></td></tr>';
    }
    h += '<tr><td><select id="remap-new">' + opts('', false) + '</select></td><td colspan="3"><a href="#" id="remap-add">add a remap</a></td></tr>';
    return h + '</table>';
  }

  function sensitivity(sc) {
    const l = layerOf(sc);
    const v = sc === 'global' ? S.input.mouse_sensitivity : (l?.mouse_sensitivity ?? null);
    const shown = v ?? S.input.mouse_sensitivity;
    return '<label>Mouse sensitivity <input type="range" id="sens" min="0.1" max="5" step="0.1" value="' + shown + '"> <span>' +
           shown.toFixed(1) + '×' + (v === null ? ' (from all games)' : '') + '</span></label>' +
           (sc !== 'global' && v !== null ? ' <a href="#" id="sens-reset">reset</a>' : '');
  }

  function actionsBlock() {
    let h = '<h4>During play</h4><div class="note">These are the client\'s, not a game\'s. A chord is held together.</div><table class="binds">';
    for (const [k, name] of [['pause', 'Pause'], ['give_up', 'Give up the run']]) {
      h += '<tr><td class="bname">' + name + '</td><td>';
      S.input.actions[k].forEach((c, i) => {
        h += '<span class="slot" data-act="' + k + '" data-i="' + i + '">' + esc(chordLabel(c)) +
             '<span class="x" data-act="' + k + '" data-i="' + i + '" title="Remove">×</span></span> ';
      });
      h += '<span class="slot add" data-act="' + k + '" data-i="' + S.input.actions[k].length + '">+</span></td></tr>';
    }
    return h + '</table><div class="note">F12 always quits the game window. In the pause menu a pad moves with the d-pad, picks with South, and resumes with Start.</div>';
  }

  function renderControls() {
    const g = scopeGame(scope);
    $('s-scopes').innerHTML = scopeList();
    let h = '';
    if (scope === 'global') {
      h += '<p class="note">Bound here, a button applies to every game, and a core or a game changes only what it overrides. ' +
           'Keys and pad inherit separately, so moving a key never moves a pad button. ' +
           'A two-button console keeps its own pad layout for any face button whose pad binds you have not changed here.</p>';
    } else if (scope.startsWith('core:')) {
      h += '<p class="note">Every game on <b>' + esc(coreName(scope.slice(5))) + '</b>, over what all games use.</p>';
    } else {
      h += '<p class="note"><b>' + esc(g.title) + '</b> only (' + esc(g.console.toUpperCase()) + ', ' + esc(coreName(g.core)) + ').</p>';
    }
    h += padSvg(g);
    h += stickTable(g);
    h += bindTable(g);
    if (g?.controller === 'keyboard' && scope.startsWith('game:')) h += remapTable(g);
    if (scope === 'global' || g?.controller === 'keyboard') h += '<div class="row2">' + sensitivity(scope) + '</div>';
    if (scope === 'global') {
      h += '<div class="row2"><label>Stick deadzone <input type="range" id="dz" min="0.1" max="0.95" step="0.05" value="' + S.input.deadzone +
           '"> <span>' + Math.round(S.input.deadzone * 100) + '%</span></label></div>';
      h += actionsBlock();
    }
    if (scope !== 'global') h += '<p><a href="#" id="scope-reset">Reset everything ' + (scope.startsWith('core:') ? 'this core' : 'this game') + ' changes</a></p>';
    $('s-body').innerHTML = h;
  }

  // ---- render: video ---------------------------------------------------------------------
  function renderVideo() {
    let h = '<div class="scope' + (vscope === 'global' ? ' on' : '') + '" data-vscope="global">All cores</div>';
    for (const c of cores()) h += '<div class="scope' + (vscope === c ? ' on' : '') + '" data-vscope="' + esc(c) + '">' + esc(coreName(c)) + '</div>';
    $('s-scopes').innerHTML = h;
    const G = S.video.global;
    const L = vscope === 'global' ? null : (S.video.cores[vscope] || {});
    const val = k => (L && L[k] !== undefined) ? L[k] : G[k];
    const pick = (k, options) => {
      let o = '';
      if (L) o += '<option value=""' + (L[k] === undefined ? ' selected' : '') + '>— as all cores (' + esc(options.find(x => x[0] == G[k])?.[1] || G[k]) + ')</option>';
      for (const [v, t] of options) o += '<option value="' + v + '"' + (L && L[k] === undefined ? '' : (val(k) == v ? ' selected' : '')) + '>' + esc(t) + '</option>';
      return '<select class="v" data-k="' + k + '">' + o + '</select>';
    };
    let b = '<p class="note">' + (L ? 'Only <b>' + esc(coreName(vscope)) + '</b>. Anything left "as all cores" follows the global setting.'
      : 'Set once for every core. A core can change any of it.') + '</p><table class="form">';
    if (!L) b += '<tr><td>Display</td><td><select id="display"><option value="windowed"' + (S.video.display === 'windowed' ? ' selected' : '') +
      '>Windowed</option><option value="borderless"' + (S.video.display === 'borderless' ? ' selected' : '') + '>Borderless fullscreen</option></select></td></tr>';
    b += '<tr><td>Window size</td><td>' + pick('window_scale', [1, 2, 3, 4, 5, 6].map(n => [n, n + '× the picture'])) + ' <span class="sub" id="vsize"></span></td></tr>';
    b += '<tr><td>Scaling</td><td>' + pick('scaling', [['fit', 'Fit — as large as fits'], ['integer', 'Integer — whole multiples only, sharpest'], ['stretch', 'Stretch — fill the window']]) + '</td></tr>';
    b += '<tr><td>Aspect</td><td>' + pick('aspect', [['core', 'As the console drew it (4:3 TV)'], ['square', 'Square pixels']]) + '</td></tr>';
    const crop = val('crop');
    b += '<tr><td>Crop edges</td><td>' + ['top', 'bottom', 'left', 'right'].map(e =>
      e + ' <input type="number" class="crop" data-e="' + e + '" min="0" max="64" value="' + crop[e] + '">').join(' ') +
      (L && L.crop !== undefined ? ' <a href="#" id="crop-reset">reset</a>' : '') + ' px</td></tr></table>';
    b += '<p class="note">Not offered, on purpose: speed, frame limiting, vsync, run-ahead and frame skip change how many emulated frames a piece of play takes or what a frame shows a player in time — and every score is counted in frames. ' +
         'Core options (filters, palettes, region) are fixed by each game\'s manifest so every player runs the same machine.</p>';
    $('s-body').innerHTML = b;
    // Size preview.
    const consoles = vscope === 'global' ? [...new Set(meta.games.map(g => g.console))] : [...new Set(coreGames(vscope).map(g => g.console))];
    const k = val('window_scale'), asp = val('aspect');
    $('vsize').textContent = consoles.filter(c => PICTURE[c]).map(c => {
      const [w, hgt] = PICTURE[c];
      const pw = w - crop.left - crop.right, ph = hgt - crop.top - crop.bottom;
      const da = asp === 'square' ? pw / ph : (4 / 3) * (hgt / w) * (pw / ph);
      return c.toUpperCase() + ' ≈ ' + Math.round(ph * k * da) + '×' + (ph * k);
    }).join(' · ');
  }

  function render() {
    if (!S || !meta) return;
    for (const t of ['controls', 'video', 'profile']) $('tab-' + t).classList.toggle('on', tab === t);
    $('s-scopes').classList.toggle('hidden', tab === 'profile');
    if (tab === 'controls') renderControls();
    else if (tab === 'video') renderVideo();
    else {
      $('s-body').innerHTML = '<table class="form"><tr><td>Display name</td><td><input id="pname" maxlength="24" value="' + esc(S.name) + '"></td></tr>' +
        '<tr><td>Server</td><td><input id="pserver" maxlength="255" spellcheck="false" placeholder="host:port or wss://host" value="' + esc(S.server || '') + '"></td></tr>' +
        '<tr><td>Stream</td><td><input id="pstream" type="number" min="100" max="2000" step="50" value="' + S.stream_kbps + '"> kbit/s</td></tr>' +
        '<tr><td>Bout input delay</td><td><input id="pdelay" type="number" min="0" max="10" value="' + (S.bout_input_delay ?? 2) + '"> frames</td></tr></table>' +
        '<p class="note">The <code>gamertrials-server</code> that Host game opens a lobby on, and that a bare invite code joins. ' +
        'Leave it blank to play on this machine alone. Without a port it means 7878, or 443 for <code>wss://</code>, which is a server behind TLS (ADR 0033).</p>' +
        '<p class="note">Stream is the picture you send a player watching you, 100 to 2000 kbit/s; 500 unless you change it. ' +
        'Higher is sharper and needs more of your upload. A link that cannot carry it drops pictures rather than falling behind, ' +
        'so too high costs a watcher smoothness, never the match. Takes effect from the next picture, even while someone watches.</p>' +
        '<p class="note">Settings are saved to <code>' + esc(settingsPath || 'nowhere — no config directory') + '</code>. They never leave this machine.</p>';
    }
  }

  // ---- events -------------------------------------------------------------------------
  $('s-scopes').onclick = e => {
    const el = e.target.closest('[data-scope],[data-vscope]');
    if (!el) return;
    if (el.dataset.scope) { scope = el.dataset.scope; sel = null; selIn = null; }
    else vscope = el.dataset.vscope;
    render();
  };
  for (const t of ['controls', 'video', 'profile']) $('tab-' + t).onclick = () => { tab = t; render(); };

  $('s-body').onclick = async e => {
    const t = e.target;
    if (t.classList.contains('x')) {
      e.stopPropagation();
      if (t.dataset.act) { S.input.actions[t.dataset.act].splice(+t.dataset.i, 1); save(); }
      else clearSlot(t.dataset.b, t.dataset.f, +t.dataset.i);
      return;
    }
    if (t.classList.contains('slot')) {
      if (t.dataset.act) {
        const got = await capture(true);
        if (got) { S.input.actions[t.dataset.act][+t.dataset.i] = got; save(); }
      } else bindSlot(t.dataset.b, t.dataset.f, +t.dataset.i);
      return;
    }
    if (t.dataset.b && (t.classList.contains('btn') || t.classList.contains('lbl'))) {
      sel = t.dataset.b; selIn = null; render();
      document.querySelector('tr[data-row="' + sel + '"]')?.scrollIntoView({ block: 'nearest' });
      return;
    }
    if (t.classList.contains('dir')) {
      selIn = t.dataset.in; sel = null; render();
      document.querySelector('select.stickto[data-in="' + selIn + '"]')?.focus();
      return;
    }
    if (t.classList.contains('reset')) { e.preventDefault(); resetOwn(scope, t.dataset.b); save(); return; }
    if (t.id === 'scope-reset') {
      e.preventDefault();
      if (scope.startsWith('core:')) delete S.cores[scope.slice(5)]; else delete S.games[scope.slice(5)];
      save(); return;
    }
    if (t.id === 'sens-reset') { e.preventDefault(); delete layerOf(scope).mouse_sensitivity; save(); return; }
    if (t.id === 'crop-reset') { e.preventDefault(); delete S.video.cores[vscope].crop; save(); return; }
    if (t.classList.contains('remap-del')) { e.preventDefault(); delete S.games[scopeGame(scope).id].key_remap[t.dataset.from]; save(); return; }
    if (t.id === 'remap-add') {
      e.preventDefault();
      const from = $('remap-new').value;
      const l = layerOf(scope, true); l.key_remap = l.key_remap || {};
      if (!(from in l.key_remap)) l.key_remap[from] = from;
      save(); return;
    }
  };

  $('s-body').onchange = e => {
    const t = e.target;
    if (t.classList.contains('stickto')) {
      if (scopeGame(scope)?.controller === 'keyboard') bindStickKey(t.dataset.in, t.value); else bindStick(t.dataset.in, t.value);
      return;
    }
    if (t.classList.contains('padkey')) {
      const l = layerOf(scope, true); l.pad_keys = l.pad_keys || {};
      l.pad_keys[t.dataset.b] = t.value;
    } else if (t.classList.contains('remap-to')) {
      S.games[scopeGame(scope).id].key_remap[t.dataset.from] = t.value;
    } else if (t.id === 'sens') {
      if (scope === 'global') S.input.mouse_sensitivity = +t.value; else layerOf(scope, true).mouse_sensitivity = +t.value;
    } else if (t.id === 'dz') {
      S.input.deadzone = +t.value;
    } else if (t.id === 'display') {
      S.video.display = t.value;
    } else if (t.classList.contains('v')) {
      const k = t.dataset.k;
      const v = k === 'window_scale' && t.value !== '' ? +t.value : t.value;
      if (vscope === 'global') S.video.global[k] = v;
      else {
        const L = S.video.cores[vscope] = S.video.cores[vscope] || {};
        if (v === '') delete L[k]; else L[k] = v;
      }
    } else if (t.classList.contains('crop')) {
      const v = Math.max(0, Math.min(64, parseInt(t.value) || 0));
      if (vscope === 'global') S.video.global.crop[t.dataset.e] = v;
      else {
        const L = S.video.cores[vscope] = S.video.cores[vscope] || {};
        L.crop = L.crop || { ...S.video.global.crop };
        L.crop[t.dataset.e] = v;
      }
    } else if (t.id === 'pname') {
      S.name = t.value;
    } else if (t.id === 'pserver') {
      S.server = t.value.trim();
    } else if (t.id === 'pstream') {
      S.stream_kbps = parseInt(t.value) || 500;
    } else if (t.id === 'pdelay') {
      S.bout_input_delay = Math.max(0, Math.min(10, parseInt(t.value) || 0));
    } else return;
    save();
  };

  // ---- live preview ----------------------------------------------------------------------
  async function poll() {
    if (capturing || tab !== 'controls') return;
    try { held = await invoke('input_preview'); } catch { held = []; }
    for (const el of document.querySelectorAll('#s-body .btn')) {
      el.classList.toggle('lit', resolveBoth(scope, el.dataset.b).some(i => held.includes(i)));
    }
    for (const el of document.querySelectorAll('#s-body .dir')) el.classList.toggle('lit', held.includes(el.dataset.in));
    for (const el of document.querySelectorAll('#s-body .slot[data-f]')) {
      const inp = resolve(scope, el.dataset.b, el.dataset.f).list[+el.dataset.i];
      el.classList.toggle('lit', inp !== undefined && held.includes(inp));
    }
  }

  // ---- open and close -------------------------------------------------------------------------
  window.openSettings = () => {
    $('menu').classList.add('hidden');
    $('settings').classList.remove('hidden');
    render();
    if (!timer) timer = setInterval(poll, 100);
  };
  $('s-back').onclick = () => {
    $('settings').classList.add('hidden');
    $('menu').classList.remove('hidden');
    clearInterval(timer); timer = null;
  };
  $('settings-open').onclick = () => window.openSettings();
  $('name').onchange = () => { if (S) { S.name = $('name').value; save(); } };

  (async () => {
    try {
      const got = await invoke('get_settings');
      meta = await invoke('settings_meta');
      S = got.settings;
      settingsPath = got.path;
      $('name').value = S.name;
      if (got.warning) $('notice').textContent = got.warning;
    } catch (e) {
      $('notice').textContent = 'cannot read settings: ' + e;
    }
  })();
})();
