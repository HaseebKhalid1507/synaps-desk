#!/usr/bin/env python3
"""Reproduces every colour number in design.md §1.6 / audit.md.

Reads the 20 built-in themes from synaps-desktop's palettes.rs and mirrors
theme.rs (`mix`, `lift`, `step`, `contrast`, and the parts of `resolve` the
contract touches). Run:  python3 roles_check.py [path/to/palettes.rs]
Analysis only: it changes nothing in the app.
"""
import re, sys, statistics as st, os

# The themes of the tree you run it in (a PR's worktree), else the path given,
# else main's checkout. It prints which.
_HERE = os.path.join(os.getcwd(), 'crates/synaps-desktop/src/palettes.rs')
SRC = sys.argv[1] if len(sys.argv) > 1 else (_HERE if os.path.exists(_HERE) else os.path.expanduser(
    '~/Projects/synaps-desktop/crates/synaps-desktop/src/palettes.rs'))
PRESETS = [None, 0xe8132f, 0xf4b400, 0xcfa85a, 0x2d7bff, 0x35f2ff, 0x2bd47d]

def themes(path):
    src = open(path).read()
    out = {}
    for name, body in re.findall(r'\(\s*"([a-z-]+)",\s*Raw \{(.*?)\},\s*\)', src, re.S):
        d = {}
        for k, v in re.findall(r'(\w+): (None|Some\(0x[0-9a-f]+\)|0x[0-9a-f]+)', body):
            d[k] = None if v == 'None' else int(re.search(r'0x([0-9a-f]+)', v).group(1), 16)
        out[name] = d
    return out

def ch(c): return ((c >> 16) & 255, (c >> 8) & 255, c & 255)
def lum(c):
    f = lambda v: (v / 255 / 12.92) if v / 255 <= 0.04045 else ((v / 255 + 0.055) / 1.055) ** 2.4
    r, g, b = ch(c); return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b)
def con(a, b):
    la, lb = lum(a), lum(b); return (max(la, lb) + 0.05) / (min(la, lb) + 0.05)
def mix(a, b, t):
    A, B = ch(a), ch(b); m = [int(x + (y - x) * t + 0.5) for x, y in zip(A, B)]
    return (m[0] << 16) | (m[1] << 8) | m[2]
def lift(frm, toward, grounds, target):
    for s in range(101):
        c = mix(frm, toward, s / 100)
        if all(con(c, g) >= target for g in grounds): return c
    return toward
def step(ground, toward, target):
    out = ground
    for i in range(1, 81):
        out = mix(ground, toward, i * 0.005)
        if con(out, ground) >= target: break
    return out

SURFACE, TEXT, MARK = 1.10, 4.5, 3.0
HAIR, EDGE, FRAME, STEP = 1.14, 1.27, 1.56, 1.14   # hybrid.html --line/--line2/--line3; raised step

def step_upto(ground, toward, target, n):
    out = ground
    for i in range(1, n + 1):
        out = mix(ground, toward, i * 0.005)
        if con(out, ground) >= target: break
    return out

def roles(t, accent=None):
    ink = t['input_fg']; canvas, chrome = t['message_bg'], t['bg']
    surf = lambda c: c if con(c, canvas) >= SURFACE else step(canvas, t['claude_text'], SURFACE)
    r = dict(canvas=canvas, chrome=chrome, user=surf(t['user_bg']))
    r['tool'] = t['tool_input_bg'] if t['tool_input_bg'] is not None else step(canvas, t['claude_text'], 1.14)
    r['tool_out'] = t['tool_output_bg'] if t['tool_output_bg'] is not None else step(canvas, t['claude_text'], 1.31)
    r['card'] = step(chrome, ink, 1.32); r['raised'] = step(chrome, ink, STEP)
    you = accent if accent is not None else t['prompt_fg']
    focus = accent if accent is not None else t['border_active']
    # today
    r['add_bg_now'] = mix(r['tool_out'], t['tool_result_ok'], 0.16)
    r['del_bg_now'] = mix(r['tool_out'], t['error_color'], 0.18)
    r['hair_now'], r['edge_now'] = t['separator'], t['border']
    # contract
    r['card_hi'] = step(r['card'], ink, STEP)
    r['hair'] = lift(t['separator'], ink, [canvas, chrome], HAIR)
    r['edge'] = lift(t['border'], ink, [canvas, chrome, r['user']], EDGE)   # controls on card use frame
    r['frame'] = lift(t['border'], ink, [r['card'], r['card_hi'], canvas, chrome, r['tool_out']], FRAME)
    r['add_bg'] = step(r['tool_out'], t['tool_result_ok'], SURFACE)
    r['del_bg'] = step(r['tool_out'], t['error_color'], SURFACE)
    k = [r['card'], r['card_hi']]
    r['run_k'] = lift(t['status_streaming'], ink, k, TEXT)
    r['ok_k'] = lift(t['tool_result_ok'], ink, k, TEXT)
    r['dim_k'] = lift(t['muted'], ink, k, TEXT)
    r['prose_k'] = lift(t['claude_text'], ink, k, TEXT)
    r['run_mark_k'] = lift(t['status_streaming'], ink, k, MARK)
    r['err_mark_k'] = lift(t['error_color'], ink, k, MARK)
    r['rest_mark_k'] = lift(t['muted'], ink, k, MARK)
    r['focus_k'] = lift(focus, ink, k, MARK)
    r['you_mark_t'] = lift(you, ink, [r['tool'], r['tool_out']], MARK)
    r['ok_c'] = lift(t['tool_result_ok'], ink, [canvas, r['tool'], r['tool_out']], TEXT)
    btn = lift(lift(you, ink, [r['user'], r['card']], MARK), ink, [chrome], TEXT)
    r['btn_fill'] = btn; r['btn_hi'] = step(btn, ink, STEP)
    r['ink'] = ink
    r['you_raw'] = you
    r['focus_raw'] = focus
    # Q1 cards (polish.md §2.1–2.3, F1 adopted): rows bare on the canvas, hovered on tool_out; an opened
    # call's well on `code` (or tool_out where the ink would not read on code's washes), washes off the well.
    code = surf(t['code_bg']); r['code'] = code
    washes = lambda g: (step_upto(g, t['tool_result_ok'], SURFACE, 60), step_upto(g, t['error_color'], SURFACE, 60))
    ca, cd = washes(code)
    reads = all(con(ink, c) >= TEXT for c in (code, ca, cd))
    r['well'] = code if reads else r['tool_out']
    r['well_add'], r['well_del'] = (ca, cd) if reads else washes(r['tool_out'])
    run = [canvas, r['tool'], r['tool_out']]
    for k, raw in (('prose_t', 'claude_text'), ('dim_t', 'muted'), ('arg', 'tool_param')):
        r[k] = lift(t[raw], ink, run, TEXT)
    r['res'] = lift(t['code_fg'] if reads else t['tool_result_color'], ink, [r['well']], TEXT)
    r['dim_o'] = lift(t['muted'], ink, [r['well']], TEXT)
    r['ok_o'] = lift(t['tool_result_ok'], ink, [r['well_add']], TEXT)
    r['err_o'] = lift(t['error_color'], ink, [r['well_del'], r['well']], TEXT)
    # Q3: inline code's chip is the well (paper: tool_out, not its dark code island), its text the well's.
    r['code_in'] = lift(t['code_fg'] if reads else t['tool_result_color'], ink, [r['well']], TEXT)
    # Q1.1 (Shady M1, M5): rules floored on the boxes they are drawn on. Their sources and
    # grounds are read from the tree's theme.rs (`f.at(...)`), not kept by hand (Shady L-b).
    grounds = {'canvas': canvas, 'tool': r['tool'], 'tool_out': r['tool_out'], 'well': r['well']}
    floors = {'HAIR': HAIR, 'EDGE': EDGE}
    for k, (raw, on, fl) in Q1_RULES.items():
        r[k] = lift(t[raw], ink, [grounds[g] for g in on], floors[fl])
    return r


def _q1_rules():
    """`r.<role> = f.at("<role>", t.<raw>, ink, &[<grounds>], <FLOOR>);` for the Q1.1 rules, from the
    theme.rs next to SRC."""
    theme = os.path.join(os.path.dirname(SRC), 'theme.rs')
    src = open(theme).read()
    out = {}
    for k in ('hair_w', 'hair_t', 'edge_t'):
        m = re.search(r'r\.%s = f\.at\("%s", t\.(\w+), ink, &\[([\w, ]+)\], (HAIR|EDGE)\)' % (k, k), src)
        if not m:
            sys.exit(f'roles_check: no `r.{k} = f.at(...)` in {theme}')
        out[k] = (m.group(1), [g.strip() for g in m.group(2).split(',')], m.group(3))
    return out


Q1_RULES = _q1_rules()

def pressed(rest, hover, toward, texts, marks):
    """theme.rs `pressed` (fluid.md §3.9): a step off the hover fill toward the ink; where that is blocked
    short of PRESS_MIN off both hover and rest, back toward rest, stopping where it stands farthest off both."""
    ok = lambda c: all(con(t, c) >= TEXT for t in texts) and all(con(m, c) >= MARK for m in marks)
    apart = lambda c: min(con(c, hover), con(c, rest))
    side = 0xffffff if lum(toward) > lum(hover) else 0x000000
    near = hover
    for i in range(1, 201):
        c = mix(hover, side, i * 0.005)
        if not ok(c): break
        near = c
        if con(c, hover) >= STEP: break
    if apart(near) >= PRESS_MIN: return near
    away = 0x000000 if lum(toward) > lum(hover) else 0xffffff
    far = hover
    for i in range(1, 81):
        c = mix(hover, away, i * 0.005)
        if not ok(c) or apart(c) < apart(far): break
        far = c
    return far if apart(far) > apart(near) else near

def drain(c, t=0.7):
    """theme.rs `drain_hsla` (motion.md §5.3): toward the grey of its own luminance, in linear light."""
    lin = lambda v: (v / 12.92) if v <= 0.04045 else ((v + 0.055) / 1.055) ** 2.4
    enc = lambda l: l * 12.92 if l <= 0.0031308 else 1.055 * l ** (1 / 2.4) - 0.055
    r, g, b = (lin(v / 255) for v in ch(c)); y = 0.2126 * r + 0.7152 * g + 0.0722 * b
    q = [min(255, max(0, int(enc(v + (y - v) * t) * 255 + 0.5))) for v in (r, g, b)]
    return (q[0] << 16) | (q[1] << 8) | q[2]

PRESS_MIN = 1.05

def press_roles(t, r):
    """The press fills exactly as theme.rs `resolve` builds them (fluid.md §3.9): the same hover
    fills, and every text and mark it floors on each."""
    ink, chrome, raised = r['ink'], r['chrome'], r['raised']
    you, focus = r['you_raw'], r['focus_raw']
    chg = [chrome, raised]
    T = lambda raw: lift(raw, ink, chg, TEXT)
    M = lambda raw: lift(raw, ink, chg, MARK)
    texts_ch = [ink] + [T(t[k]) for k in ('claude_text', 'muted', 'header_fg', 'warning_color', 'status_streaming', 'claude_label')] \
        + [T(focus)] + [T(t[k]) for k in ('status_ready', 'error_color', 'cost_color', 'tool_result_ok')]
    marks_ch = [M(t[k]) for k in ('warning_color', 'status_streaming', 'claude_label')] + [M(focus)] \
        + [M(t[k]) for k in ('status_ready', 'error_color', 'muted')] + [M(you)]
    k = [r['card'], r['card_hi']]
    texts_k = [ink, r['prose_k'], r['dim_k'], lift(t['warning_color'], ink, k, TEXT), lift(t['error_color'], ink, k, TEXT), r['run_k'], r['ok_k']]
    warn_mark = lift(t['warning_color'], ink, [r['canvas'], r['tool'], r['tool_out'], r['user'], r['card'], r['card_hi']], MARK)
    marks_k = [r['run_mark_k'], r['err_mark_k'], r['rest_mark_k'], r['focus_k'], warn_mark]
    warn_fill = lift(lift(t['warning_color'], ink, [r['user'], r['card']], MARK), ink, [chrome], TEXT)
    warn_hi = step(warn_fill, ink, STEP)
    return {
        'press_ch': (pressed(chrome, raised, ink, texts_ch, marks_ch), raised, texts_ch, marks_ch, chrome),
        'press_k': (pressed(r['card'], r['card_hi'], ink, texts_k, marks_k), r['card_hi'], texts_k, marks_k, r['card']),
        'btn_press': (pressed(r['btn_fill'], r['btn_hi'], ink, [chrome], []), r['btn_hi'], [chrome], [], r['btn_fill']),
        'warn_press': (pressed(warn_fill, warn_hi, ink, [chrome], []), warn_hi, [chrome], [], warn_fill),
    }

def main():
    T = themes(SRC)
    print(f'{len(T)} themes from {SRC}\n')
    rows = []
    for n, t in T.items():
        for a in PRESETS:
            rows.append((n, a, roles(t, a)))
    base = {n: r for n, a, r in rows if a is None}

    def line(name, vals):
        print(f'{name:44s} min {min(vals):5.2f}  median {st.median(vals):5.2f}  max {max(vals):5.2f}')

    print('== Today (no accent)')
    line('diff add wash vs canvas', [con(r['add_bg_now'], r['canvas']) for r in base.values()])
    line('diff del wash vs canvas', [con(r['del_bg_now'], r['canvas']) for r in base.values()])
    line('card vs canvas', [con(r['card'], r['canvas']) for r in base.values()])
    print('  themes where a wash is louder than the card:',
          sum(1 for r in base.values() if max(con(r['add_bg_now'], r['canvas']), con(r['del_bg_now'], r['canvas'])) > con(r['card'], r['canvas'])), '/ 20')
    line('edge (raw border) vs card', [con(r['edge_now'], r['card']) for r in base.values()])
    print('  hair (raw separator) under 1.14 on canvas or chrome:',
          sorted(n for n, r in base.items() if min(con(r['hair_now'], r['canvas']), con(r['hair_now'], r['chrome'])) < HAIR))
    line('card vs tool_out', [con(r['card'], r['tool_out']) for r in base.values()])
    print('  raw accent under 3:1 on card (20 themes x 6 presets):',
          sum(1 for n, a, r in rows if a and con(r['you_raw'], r['card']) < MARK), 'of', sum(1 for n, a, r in rows if a))
    print('  run == warn (raw):', sorted(n for n, t in T.items() if t['status_streaming'] == t['warning_color']))
    print('  flush chrome (bg == message_bg):', sum(1 for t in T.values() if t['bg'] == t['message_bg']), '/ 20')
    print('  light themes (canvas luminance > 0.4):', sorted(n for n, t in T.items() if lum(t['message_bg']) > 0.4))

    print('\n== Contract (all 140 theme x accent cases)')
    allr = [r for _, _, r in rows]
    line('hair vs canvas/chrome (floor 1.14)', [min(con(r['hair'], r['canvas']), con(r['hair'], r['chrome'])) for r in allr])
    line('edge vs canvas/chrome/user (1.27)', [min(con(r['edge'], g) for g in (r['canvas'], r['chrome'], r['user'])) for r in allr])
    print('  themes whose edge is lifted:', sum(1 for n, r in base.items() if r['edge'] != r['edge_now']), '/ 20')
    line('frame vs card/card_hi/tool_out (1.56)', [min(con(r['frame'], g) for g in (r['card'], r['card_hi'], r['tool_out'])) for r in allr])
    line('card_hi vs card', [con(r['card_hi'], r['card']) for r in allr])
    line('diff add wash vs tool_out (1.10)', [con(r['add_bg'], r['tool_out']) for r in allr])
    line('diff add wash vs canvas', [con(r['add_bg'], r['canvas']) for r in allr])
    line('diff del wash vs canvas', [con(r['del_bg'], r['canvas']) for r in allr])
    print('  themes where a wash is louder than the card by > 0.15:',
          sum(1 for r in base.values() if max(con(r['add_bg'], r['canvas']), con(r['del_bg'], r['canvas'])) > con(r['card'], r['canvas']) + 0.15), '/ 20')
    line('ink on add/del wash', [min(con(r['ink'], r['add_bg']), con(r['ink'], r['del_bg'])) for r in allr])
    line('ink on card_hi', [con(r['ink'], r['card_hi']) for r in allr])
    for k in ('run_k', 'ok_k', 'dim_k', 'prose_k'):
        line(f'{k} on card & card_hi (4.5)', [min(con(r[k], r['card']), con(r[k], r['card_hi'])) for r in allr])
    for k in ('run_mark_k', 'err_mark_k', 'rest_mark_k', 'focus_k'):
        line(f'{k} on card & card_hi (3.0)', [min(con(r[k], r['card']), con(r[k], r['card_hi'])) for r in allr])
    line('you_mark_t on tool & tool_out (3.0)', [min(con(r['you_mark_t'], r['tool']), con(r['you_mark_t'], r['tool_out'])) for r in allr])
    line('chrome label on btn_hi (4.5)', [con(r['chrome'], r['btn_hi']) for r in allr])
    line('chrome label on btn_fill today', [con(r['chrome'], r['btn_fill']) for r in allr])
    moved = sorted(n for n, r in base.items() if r['ok_c'] != lift(T[n]['tool_result_ok'], r['ink'], [r['tool'], r['tool_out']], TEXT))
    print('  ok: themes that change when canvas joins its grounds:', moved or 'none')

    print('\n== Q1 cards (all 140 theme x accent cases): rows bare, hover fill, wells')
    fails = []
    def floor(name, vals, f):
        line(f'{name} ({f})', vals)
        if min(vals) < f - 1e-9: fails.append(name)
    floor('hover fill tool_out vs canvas', [con(r['tool_out'], r['canvas']) for r in allr], SURFACE)
    for k in ('prose_t', 'dim_t', 'arg'):
        floor(f'{k} on canvas & hover fill', [min(con(r[k], r['canvas']), con(r[k], r['tool_out'])) for r in allr], TEXT)
    floor('well vs canvas', [con(r['well'], r['canvas']) for r in allr], SURFACE)
    floor('hair frame vs canvas', [con(r['hair'], r['canvas']) for r in allr], HAIR)
    floor('add wash vs well', [con(r['well_add'], r['well']) for r in allr], SURFACE)
    floor('del wash vs well', [con(r['well_del'], r['well']) for r in allr], SURFACE)
    floor('ink on add/del wash', [min(con(r['ink'], r['well_add']), con(r['ink'], r['well_del'])) for r in allr], TEXT)
    floor('ok_o sign on add wash', [con(r['ok_o'], r['well_add']) for r in allr], TEXT)
    floor('err_o on del wash & well', [min(con(r['err_o'], r['well_del']), con(r['err_o'], r['well'])) for r in allr], TEXT)
    floor('res (well text) on well', [con(r['res'], r['well']) for r in allr], TEXT)
    floor('dim_o (numbers, header) on well', [con(r['dim_o'], r['well']) for r in allr], TEXT)
    print('  themes whose well stays on tool_out:', sorted(n for n, r in base.items() if r['well'] != r['code']) or 'none')
    print('  (the text pairs above are floored on those grounds by construction: they fail only if lift saturates)')
    print('\n  -- Q1 drawn pairs: each colour on the ground it is painted over (theme.rs q1_draws_every_pair_above_its_floor)')
    floor('well header rule hair_w on well', [con(r['hair_w'], r['well']) for r in allr], HAIR)
    floor('well frame hair_w on canvas', [con(r['hair_w'], r['canvas']) for r in allr], HAIR)
    floor('chip frame hair_t on tool/tool_out/canvas', [min(con(r['hair_t'], r[g]) for g in ('tool', 'tool_out', 'canvas')) for r in allr], HAIR)
    floor('OPEN outline edge_t on canvas/hover fill', [min(con(r['edge_t'], r[g]) for g in ('canvas', 'tool_out')) for r in allr], EDGE)
    floor('OPEN hovered outline frame on hover fill', [con(r['frame'], r['tool_out']) for r in allr], FRAME)
    floor('OPEN hovered word prose_t on hover fill', [con(r['prose_t'], r['tool_out']) for r in allr], TEXT)
    print('  -- what 75fc7d8 drew there (report; the reason for the roles above)')
    line('  hair on well (header rule)', [con(r['hair'], r['well']) for r in allr])
    line('  hair on tool (chip frame)', [con(r['hair'], r['tool']) for r in allr])
    line('  edge on tool_out (OPEN, hovered row)', [con(r['edge'], r['tool_out']) for r in allr])
    print('  themes with hair on well under 1.14:', sorted(n for n, r in base.items() if con(r['hair'], r['well']) < HAIR))
    line('  well vs tool_out (a hovered row over its well; a frame between)', [con(r['well'], r['tool_out']) for r in allr])
    floor('code_in (inline code) on its chip, the well', [con(r['code_in'], r['well']) for r in allr], TEXT)
    floor('inline code chip (well) vs canvas', [con(r['well'], r['canvas']) for r in allr], SURFACE)
    # Q3's own chip (well != code: paper) on a user bubble. Where the chip is the code surface (main's
    # behaviour) it may equal the user surface, as on main: reported, not floored.
    floor('inline code chip (well != code) vs the user bubble', [con(r['well'], r['user']) for r in allr if r['well'] != r['code']], SURFACE)
    line('  chip = code surface vs the user bubble (main)', [con(r['well'], r['user']) for r in allr if r['well'] == r['code']])
    print('  paper inline chip #%06x (lum %.2f), text #%06x' % (base['paper']['well'], lum(base['paper']['well']), base['paper']['code_in']))
    print('  Q1 floors:', 'FAIL ' + ', '.join(fails) if fails else 'all pass')

    print('\n== Q4 press (fluid.md §3.9) and drain (motion.md §5.3), all 140 cases')
    q4 = []
    def q4floor(name, vals, f):
        line(f'{name} ({f})', vals)
        if min(vals) < f - 1e-9: q4.append(name)
    press = [(t, r, press_roles(T[n], r)) for n, a, r in rows for t in [T[n]]]
    for k in ('press_ch', 'press_k', 'btn_press', 'warn_press'):
        q4floor(f'{k} vs its hover fill', [con(pr[k][0], pr[k][1]) for _, _, pr in press], PRESS_MIN)
        q4floor(f'{k} vs its rest fill', [con(pr[k][0], pr[k][4]) for _, _, pr in press], PRESS_MIN)
        q4floor(f'texts on {k}', [min(con(x, pr[k][0]) for x in pr[k][2]) for _, _, pr in press], TEXT)
        if k in ('press_ch', 'press_k'):
            q4floor(f'marks on {k}', [min(con(x, pr[k][0]) for x in pr[k][3]) for _, _, pr in press], MARK)
    print('  press stepped away from the ink (toward was blocked):',
          sum(1 for _, r, pr in press for k in pr if (lum(pr[k][0]) - lum(pr[k][1])) * (lum(r['ink']) - lum(pr[k][1])) < 0), 'of', 4 * len(press))
    print('  press == hover anywhere:', sum(1 for _, _, pr in press for k in pr if pr[k][0] == pr[k][1]))
    # The drain keeps luminance, so every pair keeps its contrast (8-bit rounding: 0.03 allowed).
    TOL = 0.03
    q4floor('drained run_k/ok_k on card & card_hi', [min(con(drain(r[k]), g) for k in ('run_k', 'ok_k') for g in (r['card'], r['card_hi'])) for r in allr], TEXT - TOL)
    q4floor('drained marks on card & card_hi', [min(con(drain(r[k]), g) for k in ('run_mark_k', 'err_mark_k', 'focus_k') for g in (r['card'], r['card_hi'])) for r in allr], MARK - TOL)
    q4floor('drained you_mark_t on tool & tool_out', [min(con(drain(r['you_mark_t']), g) for g in (r['tool'], r['tool_out'])) for r in allr], MARK - TOL)
    q4floor('chrome label on drained btn_fill/btn_hi/btn_press', [min(con(r['chrome'], drain(c)) for c in (r['btn_fill'], r['btn_hi'], pr['btn_press'][0])) for _, r, pr in press], TEXT - TOL)
    print('  Q4 floors:', 'FAIL ' + ', '.join(q4) if q4 else 'all pass')
    fails += q4

    print('\n== Q2 authorize card (polish.md §2.6; finish gate and review of #18), all 140 cases')
    q2 = []
    def q2floor(name, vals, f):
        line(f'{name} ({f})', vals)
        if min(vals) < f - 1e-9: q2.append(name)
    CMD = 7.0
    def card_k(t, r):
        k = [r['card'], r['card_hi']]
        return {'prose_k': r['prose_k'], 'dim_k': r['dim_k'], 'warn_k': lift(t['warning_color'], r['ink'], k, TEXT), 'err_k': lift(t['error_color'], r['ink'], k, TEXT)}
    def cmd(t, r):
        # theme.rs well_fg: code_fg lifted on the canvas (the well) to CMD and to every other text on the card.
        loud = max([CMD] + [con(c, r['card']) for c in card_k(t, r).values()])
        return lift(t['code_fg'], r['ink'], [r['canvas']], loud)
    q2rows = [(T[n], r) for n, a, r in rows]
    q2floor('the command (well_fg) on its well', [con(cmd(t, r), r['canvas']) for t, r in q2rows], CMD)
    q2floor('the command over the card\'s loudest other text (ratio)', [con(cmd(t, r), r['canvas']) / max(con(c, r['card']) for c in card_k(t, r).values()) for t, r in q2rows], 1.0)
    wm = lambda t, r: lift(t['warning_color'], r['ink'], [r['canvas'], r['tool'], r['tool_out'], r['user'], r['card'], r['card_hi']], MARK)
    q2floor('warn_mark frame and dashes on card', [con(wm(t, r), r['card']) for t, r in q2rows], MARK)
    q2floor('focus_k Allow all mark on card', [con(r['focus_k'], r['card']) for t, r in q2rows], MARK)
    q2floor('you ($) on the well', [con(lift(r['you_raw'], r['ink'], [r['canvas']], TEXT), r['canvas']) for t, r in q2rows], TEXT)
    errr = lambda t, r: lift(t['error_color'], r['ink'], [r['canvas'], r['tool'], r['tool_out'], r['user']], TEXT)
    q2floor('receipt "denied" (err) on the row: canvas, tool_out', [min(con(errr(t, r), g) for g in (r['canvas'], r['tool_out'])) for t, r in q2rows], TEXT)
    line('  (err_o there instead, for comparison: not used)', [min(con(r['err_o'], g) for g in (r['canvas'], r['tool_out'])) for t, r in q2rows])
    q2floor('receipt words (dim_t) on the row', [min(con(r['dim_t'], g) for g in (r['canvas'], r['tool_out'])) for t, r in q2rows], TEXT)
    print('  Q2 floors:', 'FAIL ' + ', '.join(q2) if q2 else 'all pass')
    fails += q2

    print('\n== Q6 palette (review of #20 N5; re-gate of #20 item 4), all 140 cases')
    q6 = []
    def q6floor(name, vals, f):
        line(f'{name} ({f})', vals)
        if min(vals) < f - 1e-9: q6.append(name)
    # theme.rs: thumb_k = mix(card, dim_k, 0.55), drawn on the card and over the palette's selected row (card_hi).
    thumb_k = lambda r: mix(r['card'], r['dim_k'], 0.55)
    q6floor('thumb_k on card', [con(thumb_k(r), r['card']) for r in allr], HAIR)
    q6floor('thumb_k on card_hi (the selected row)', [con(thumb_k(r), r['card_hi']) for r in allr], HAIR)
    # theme.rs: you_k = the accent (prompt_fg without one) lifted toward the ink to TEXT on card and card_hi:
    # the palette's matched letters (re-gate of #20, item 4). `you` raw, for comparison.
    you_k = lambda r: lift(r['you_raw'], r['ink'], [r['card'], r['card_hi']], TEXT)
    line('  (raw you on card_hi, for comparison: not used)', [con(r['you_raw'], r['card_hi']) for r in allr])
    q6floor('you_k (matched letters) on card', [con(you_k(r), r['card']) for r in allr], TEXT)
    q6floor('you_k (matched letters) on card_hi', [con(you_k(r), r['card_hi']) for r in allr], TEXT)
    print('  Q6 floors:', 'FAIL ' + ', '.join(q6) if q6 else 'all pass')
    fails += q6

    print('\n== nord (default theme), no accent')
    r = base['nord']
    for k in ('canvas', 'chrome', 'raised', 'user', 'tool', 'tool_out', 'card', 'card_hi', 'hair_now', 'hair', 'edge_now', 'edge', 'frame',
              'add_bg_now', 'add_bg', 'del_bg_now', 'del_bg', 'run_k', 'ok_k', 'focus_k', 'you_mark_t', 'btn_fill', 'btn_hi'):
        print(f'  {k:12s} #{r[k]:06x}  vs canvas {con(r[k], r["canvas"]):.2f}')
    return 1 if fails else 0

if __name__ == '__main__':
    sys.exit(main())
