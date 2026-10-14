#!/usr/bin/env python3
"""Extract Cadash RPG tables (classes/levels, items, equipment, shops, spells, enemies, RAM map, menus)
from the ROM into extract/out/rpg/*.json.  Evidence addresses: see docs/re/ui_rpg_data.md."""
import json, os, struct

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ROM = open(os.path.join(ROOT, 'Cadash (USA, Korea) (En).md'), 'rb').read()
OUT = os.path.join(ROOT, 'extract', 'out', 'rpg')
os.makedirs(OUT, exist_ok=True)

def be16(a): return struct.unpack('>H', ROM[a:a+2])[0]
def be32(a): return struct.unpack('>I', ROM[a:a+4])[0]
def h(v, w=0): return '0x%0*X' % (w, v)

CHR = {0x3A: ' ', 0x3B: ',', 0x3C: '.', 0x3D: '!', 0x3E: "'", 0x5B: '/', 0x5C: 'x'}
def dec(b):
    return ''.join(CHR.get(c, chr(c) if 0x20 <= c < 0x7F else '{%02X}' % c) for c in b)

def text(tid):
    """Dialogue text id (1-based) from pointer table 0x2D528, decoded up to FF/EE."""
    p = be32(0x2D528 + 4 * (tid - 1))
    out = []
    i = p
    while ROM[i] not in (0xFF, 0xEE) and i < p + 400:
        c = ROM[i]
        if c == 0xFD: out.append('\n')
        elif c == 0xFC: out.append('\n\n')
        elif c in (0xF8, 0xF7, 0xF5, 0xF4): out.append('<NAME>')
        elif c == 0xEF: out.append('<PRICE>')
        else: out.append(dec(bytes([c])))
        i += 1
    return ''.join(out).replace('{FE}', '').strip()

def dump(name, obj):
    with open(os.path.join(OUT, name), 'w') as f:
        json.dump(obj, f, indent=1)

# ---------------------------------------------------------------- classes / level tables
CLASS_NAMES = ['Fighter', 'Mage']
SPELL_MSG = {1: 'Flying Dagger', 2: 'Flame Wall', 3: 'Blizzard', 4: 'Lightning Bolt', 5: 'Explosion'}
classes = {
    'source': {
        'level_table_ptrs': '0xB896 (long per class, index = player+0x6A)',
        'record': '10 bytes, stored highest level first: rec[L] at ptr + 10*(19-L) (L 0-based)',
        'fields': {'+0 w': 'total EXP required to reach this level', '+2 b': 'max HP base (+8 per Bell of Life, cap 255)',
                   '+3 b': 'max MP', '+4 b': 'STR base (+weapon bonus)', '+5 b': 'A.C. base (+armor bonuses)',
                   '+6 b': 'AGL (also walk speed: player+0xA2 = AGL<<8 + 0x18000 Fighter / 0x16000 Mage, 16.16 px/frame)',
                   '+7 b': 'number of extra spells known (Mage); change triggers HUD msg 5+n "<spell>\'S SPELL HAS BEEN MEMORIZED" (msg ids via 0xC7AA)',
                   '+8 w': 'attack animation frame duration (player+0x9E, used by anim frames with duration 0x8000); Fighter with Sword of Speed: -1'},
        'stat_apply_routine': '0xC5B0 (init: find level from EXP) / 0xC5EC (apply record); level-up check 0xC542',
    },
    'level_cap': 20,
    'level_up': {
        'routine': '0xC542 (called after a kill at 0x147E8 with d0 = killer player struct)',
        'rule': 'if level(+0x8C,0-based) != 19 and EXP(+0x8A) >= rec[level+1].exp: level += 1 (only one level per kill), HUD msg 1 "<NAME> HAS MOVED TO A HIGHER LEVEL", sound 0x92, re-apply stats with d7=1',
        'heal_on_level_up': False,
        'note': 'with d7=1 only max HP/MP are rewritten (+0x8E/+0x94); current HP/MP (+0x90/+0x96) are NOT refilled. Next-level EXP (+0xDC) = rec[level+1].exp, 0xFFFF at level 20.',
    },
    'classes': [],
}
for c in range(2):
    p = be32(0xB896 + 4 * c)
    lv = []
    for L in range(20):
        a = p + 10 * (19 - L)
        r = ROM[a:a+10]
        lv.append({'level': L + 1, 'exp': be16(a), 'max_hp': r[2], 'max_mp': r[3], 'str': r[4], 'ac': r[5],
                   'agl': r[6], 'spells_known_extra': r[7], 'attack_frame_duration': be16(a + 8)})
    classes['classes'].append({'id': c, 'name': CLASS_NAMES[c], 'table_addr': h(p), 'levels': lv})
dump('classes.json', classes)

# ---------------------------------------------------------------- shop item records 0x33076
ITEM_REC = 0x33076
recs = []
for i in range(22):
    a = ITEM_REC + 12 * i
    recs.append({'index': i, 'addr': h(a), 'target': be32(a), 'text_id': be16(a + 4), 'class': be16(a + 6),
                 'value': be16(a + 8), 'price': be16(a + 10)})

# equipment slots and bonus tables
SLOTS = {
    0xFF0AC6: ('fighter_weapon', 'Fighter', 'STR', 0xC790, ['(none)', 'Broadsword', 'Blade of Fire', 'Sword of Speed', 'Battle Sword']),
    0xFF0AC0: ('fighter_armor', 'Fighter', 'AC', 0xC796, ['(none)', 'Chain Mail', 'Plate Armor', 'Golden Armor']),
    0xFF0AC2: ('fighter_helm', 'Fighter', 'AC', 0xC79A, ['(none)', 'Silver Helm', 'Golden Helm']),
    0xFF0AC4: ('fighter_shield', 'Fighter', 'AC', 0xC79E, ['(none)', 'Leather Shield', 'Silver Shield', 'Golden Shield']),
    0xFF0ACA: ('mage_wand', 'Mage', 'STR', 0xC7A2, ['(none)', 'Iron Wand', 'Staff of Fire', 'Staff of the Mage']),
    0xFF0AC8: ('mage_robe', 'Mage', 'AC', 0xC7A6, ['(none)', "Robe of the Quester", "Mystic's Robe"]),
}
SPECIAL = {
    ('fighter_weapon', 2): 'attacking also fires a fireball projectile (damage 8, speed 6 px/f, 0x11-frame cooldown) – 0xB692',
    ('fighter_weapon', 3): 'attack animation duration -1 frame (player+0x9E) – 0xC75E',
    ('mage_wand', 2): 'attacking also fires a fireball projectile (damage 8, speed 6 px/f) – 0xB692',
    ('mage_wand', 3): 'MP regenerates +1 every 420 (0x1A4) frames, counter 0xFF0A76 – 0xC262',
    ('fighter_shield', 1): 'any shield: blocks enemy projectiles flagged blockable (obj+0x67 bit3) hitting the facing side while not attacking – 0xA0EA',
}
equipment = {'source': {'records': '0x33076 (12 bytes: l target RAM, w text id, w class 0=Fighter 1=Mage 2=any, w value, w price)',
                        'bonus_tables': 'byte tables indexed by slot value; applied in 0xC63E (Fighter) / 0xC6CC (Mage)',
                        'formulas': {'Fighter STR': 'rec.str + [0xC790][weapon 0xFF0AC6]',
                                     'Fighter AC': 'rec.ac + [0xC796][armor 0xFF0AC0] + [0xC79A][helm 0xFF0AC2] + [0xC79E][shield 0xFF0AC4]',
                                     'Mage STR': 'rec.str + [0xC7A2][wand 0xFF0ACA]',
                                     'Mage AC': 'rec.ac + [0xC7A6][robe 0xFF0AC8]'},
                        'graphics': 'equipment changes reload weapon/body tiles (0x920C, 0x95A6) and player palette (0xC364, tables 0xB800..)'},
             'slots': []}
for ram, (slot, cls, stat, tab, names) in SLOTS.items():
    items = []
    for v, nm in enumerate(names):
        r = next((x for x in recs if x['target'] == ram and x['value'] == v), None)
        items.append({'value': v, 'name': nm, 'bonus': ROM[tab + v], 'stat': stat,
                      'price': r['price'] if r else None, 'shop_record': r['index'] if r else None,
                      'text_id': r['text_id'] if r else None,
                      'shop_text': text(r['text_id']) if r else None,
                      'special': SPECIAL.get((slot, v)) or (SPECIAL.get((slot, 1)) if slot == 'fighter_shield' and v > 0 else None)})
    equipment['slots'].append({'slot': slot, 'ram': h(ram, 6), 'class': cls, 'stat': stat, 'bonus_table': h(tab), 'items': items})
dump('equipment.json', equipment)

# ---------------------------------------------------------------- items
items = {
    'source': {'shop_records': '0x33076 (records 0..4)', 'pickup_targets': '0xA3AE (long per pickup kind-3)', 'pickup_texts': '0xA3D6 (word text id)',
               'status_names': '0x2D4FE "MEDICAL HERB", "ANTIDOTE", "ELIXIR" (14-char fields incl. "\\"(x) separator)'},
    'consumables': [
        {'name': 'Medical Herb', 'ram': 'player+0xD2', 'max': 4, 'price': 200, 'pickup_kind': 3,
         'effect': 'AUTO-USED when HP would drop to 0: HP = 30 (set, not added); HUD msg 11 "<NAME> HAS TAKEN MEDICINAL HERBS. 30 HIT POINTS ARE RESTORED."',
         'evidence': '0xA15E (enemy hit), 0xA446 (poison), 0xA544 (hazard)'},
        {'name': 'Antidote', 'ram': 'player+0xD4', 'max': 4, 'price': 40, 'pickup_kind': 5,
         'effect': 'AUTO-USED on the first poison tick (every 120 frames while poisoned): cures poison; HUD msg 12',
         'evidence': '0xA412'},
        {'name': 'Elixir (of Life)', 'ram': 'player+0xD8', 'max': 4, 'price': None, 'pickup_kind': 4,
         'effect': 'AUTO-USED when HP would drop to 0 and no herbs: HP=maxHP, MP=maxMP, cures poison; HUD msg 13',
         'evidence': '0xA17E'},
        {'name': 'Bell of Life', 'ram': 'player+0xD6', 'max': 4, 'price': 500,
         'effect': 'permanent +8 max HP each (max HP = base + 8*count, cap 255); applied immediately at purchase (0x32C24)',
         'evidence': '0xC5F6, 0x32C24'},
        {'name': 'Extra life', 'ram': '0xFF0A7A (shared lives counter, starts 2)', 'max': 9, 'price': 'dynamic 0xFF0A78: starts 800, doubles after each purchase, cap 6400',
         'effect': 'one additional life (text 138)', 'evidence': '0x32AEE, 0x32B32'},
        {'name': 'Silver Key', 'ram': '0xFF0AD2', 'max': 1, 'price': 800, 'effect': 'opens the silver door (text 113)'},
    ],
    'key_items': [],
    'poison': {'flag': 'player+0xA6', 'inflict': 'contact with enemy whose obj+0x67 bit2 set, only in room >= 6, 50% (random byte 0xFF0240 < 0x80); HUD msg 4; palette tint (0xC2D6)',
               'tick': 'every 120 frames (timer player+0xA8): antidote auto-use, else HP -= 3', 'evidence': '0xA800, 0xA3EA'},
}
for k in range(10):
    t = be32(0xA3AE + 4 * k); tid = be16(0xA3D6 + 2 * k)
    if k >= 3:
        items['key_items'].append({'pickup_kind': k + 3, 'ram': h(t, 6), 'text_id': tid, 'text': text(tid)})
dump('items.json', items)

# ---------------------------------------------------------------- shops
def shop_slots(block):
    ids = list(ROM[block + 0xAF: block + 0xBA])
    return ids
SHOP_ITEM_NAMES = {0: 'Medical Herb', 1: 'Antidote', 2: 'Bell of Life', 3: 'Extra life', 4: 'Silver Key'}
for s in equipment['slots']:
    for it in s['items']:
        if it['shop_record'] is not None: SHOP_ITEM_NAMES[it['shop_record']] = it['name']
def describe(block):
    ids = shop_slots(block)
    grid = []
    for i, v in enumerate(ids):
        if v != 0xFF:
            r = recs[v]
            grid.append({'slot_index': i, 'row': i // 4, 'col': i % 4, 'record': v, 'name': SHOP_ITEM_NAMES[v],
                         'class': ['Fighter', 'Mage', 'any'][r['class']], 'price': r['price'] if v != 3 else 'dynamic (0xFF0A78)'})
    return {'block_addr': h(block), 'items': grid}
shops = {
    'source': {'entry': 'talking (B pressed while touching) to an NPC whose obj+0x74 != 0 → 0xFF0BB6 = shop type (1 inn, 2 armory, 3 apothecary), 0xFF0BB2 = customer player struct (0xA310)',
               'dispatch': '0x321FC → jump table 0x32236 (inn 0x32242, armory/apothecary 0x32482)',
               'npc_records': 'room object table 0x1CCAC → list 0x1CF6C entries 0/1/2 = inn/armory/apothecary NPC (records 0x1E3C8/0x1E3DE/0x1E3F4); room 40 uses list 0x1CFAC (inn = index 3)',
               'placement': 'map spawn list 0x1C6D6[room] (x,y,id) bytes',
               'shop_block': '0xBA bytes: 17x5 tilemap words (item icons + prices, tiles 0x200+) drawn at window cell (6,2) by 0x109A8, +0xAF: 11-byte slot grid (record index or 0xFF), 4 slots per row'},
    'inn': {'price_by_room': {'29': 100, '35': 200, '40': 500, 'default': 100}, 'price_ram': 'customer+0xD0 (set on entry, doubled (cap 1000) after paying but reset on next visit)',
            'text': {'greeting': 95, 'rest': 96, 'no_money': 97, 'declined': 98, 'morning': 99},
            'effect': 'gold -= price; HP=maxHP, MP=maxMP; screen fade, 60-frame pause, jingle 0x94', 'evidence': '0x32242..0x32480'},
    'armory': {'text': {'greeting': 100, 'what': 101, 'else': 102, 'bye': 103, 'no_money': 104, 'who_for_2p': 105, 'wrong_class': 106}},
    'apothecary': {'text': {'greeting': 107, 'bye': 108, 'no_money': 109, 'too_many': 110}},
    'shops': [
        {'room': 29, 'type': 'inn', 'price': 100},
        {'room': 30, 'type': 'armory', **describe(0x3317E)},
        {'room': 32, 'type': 'apothecary', **describe(0x33520)},
        {'room': 33, 'type': 'armory', **describe(0x33238)},
        {'room': 34, 'type': 'armory', **describe(0x332F2)},
        {'room': 35, 'type': 'inn', 'price': 200},
        {'room': 36, 'type': 'apothecary', **describe(0x33694)},
        {'room': 39, 'type': 'armory', **describe(0x333AC)},
        {'room': 40, 'type': 'inn', 'price': 500},
        {'room': 42, 'type': 'armory', **describe(0x33466)},
        {'room': 43, 'type': 'apothecary', **describe(0x335DA)},
    ],
    'non_shop_house_rooms': {'31': 'talker (text 23)', '37': 'talker (text 40)', '41': 'talker (text 55)', '38': 'empty'},
    'buy_flow': [
        'greeting text with YES/NO (EE) – window 4 (28x16 tiles @ cell 6,4); choose with UP=YES / DOWN=NO, B confirms, A/C = NO',
        'YES → text 101 "WHAT WOULD YOU LIKE?" and item grid; cursor (tile 0x828A/arrow 0xC24D P1, 0xC24F P2) starts at window cell (5,2); D-pad moves: UP/DOWN = slot -/+4 (3 tile rows), LEFT/RIGHT = slot -/+1 (10 tile cols); moves onto 0xFF slots are refused; sound 0xB4 per move',
        'B on item → (2P: "WHO IS THIS FOR?" pick player name with UP/DOWN) → checks in order: class (rec.class must be 2 or buyer class, else text 106 wrong_class), count/level (*target >= rec.value → text 110 too many; for equipment you cannot buy an item of equal/lower tier than equipped), gold (gold < price → armory 104 / apothecary 109)',
        'purchase: gold -= price, sound 0x9C; consumables (records 0-2): count += 1 (Bell also maxHP += 8 cap 255); extra life: lives += 1 and price doubles (cap 6400); equipment: slot = value (old item discarded), reload gfx/palette and recompute STR/AC (0xC5B0 with 0xFF0A70=1, HP/MP untouched); gold box refreshed (0x32D2C)',
        'then text 102 "WOULD YOU LIKE SOMETHING ELSE?" YES/NO; A or C in the grid / NO → text 103 (armory) / 108 (apothecary) and exit',
    ],
}
dump('shops.json', shops)

# ---------------------------------------------------------------- spells
costs = [be16(0xAB40 + 2 * i) for i in range(6)]
spells = {
    'source': {'cost_table': '0xAB40 (6 words, index player+0xA0)', 'cast_input': '0xA9F0', 'launch_dispatch': '0xB1E4 → table 0xB1F8',
               'update_dispatch': '0xAC5E → table 0xAC72', 'objects': 'spell projectiles at 0xFFB824.. (stride 0x100), damage = obj+0x66 subtracted directly from enemy HP (no defense) at 0x1478E',
               'icon': 'selection icon object 0xFFB7A4 drawn 0x38 px above the mage, mappings 0xE2A6[spell], palette 3 words 0xE3CA + 6*spell'},
    'input': 'Mage only (player+0x6A==1). Hold B: after 30 frames (counter 0xFF0B5C) the spell icon appears (0xFF0B5E=2); keep holding: every 30 frames the selection cycles 0..spells_known (player+0x9C) and wraps; release B: if MP (+0x96) >= cost → MP -= cost and cast (0xFF0B60 = spell, 0xFF0B66 = active); else HUD msg 5 "YOU DON\'T HAVE ENOUGH MP LEFT." Releasing before the icon appears = normal attack. Only one spell active at a time.',
    'learn_rule': 'spells available = indices 0..level_table.spells_known_extra (Mage: L1-2: 0, L3: 1, L6: 2, L8: 3, L16: 4, L20: 5)',
    'spells': [
        {'index': 0, 'name': '(basic magic – unnamed in ROM)', 'learned_at_level': 1, 'mp': costs[0], 'damage': 20, 'launch': '0xB210',
         'behaviour': 'one projectile, horizontal 6 px/frame in facing direction, mapping 0xE3EE[facing]'},
        {'index': 1, 'name': 'Flying Dagger', 'learned_at_level': 3, 'mp': costs[1], 'damage': 40, 'launch': '0xB288', 'update': '0xACE4',
         'behaviour': '8 daggers launched in 8 directions (velocity table 0xB310), each frame-pair accelerates 0x7000 toward the caster (max 7 px/f) so they swarm around him; lasts 180 frames'},
        {'index': 2, 'name': 'Flame Wall', 'learned_at_level': 6, 'mp': costs[2], 'damage': 20, 'launch': '0xB350', 'update': '0xAE16',
         'behaviour': 'needs the caster on ground; fire spawned 24 px in front (snapped to 8 px), spreads along the floor in 16 px steps (0xFF0B6E) [partially decoded]'},
        {'index': 3, 'name': 'Blizzard', 'learned_at_level': 8, 'mp': costs[3], 'damage': 40, 'launch': '0xB458', 'update': '0xAF44',
         'behaviour': '8 ice shards (angle index obj+0x68 = 4*i) circling the caster, mappings 0xE4C6 [partially decoded]'},
        {'index': 4, 'name': 'Lightning Bolt', 'learned_at_level': 16, 'mp': costs[4], 'damage': 20, 'launch': '0xB4FC', 'update': '0xB032',
         'behaviour': '16 bolt segments, direction from facing (7 or 1), sound 0xAC, mappings 0xE4A2 [partially decoded]'},
        {'index': 5, 'name': 'Explosion', 'learned_at_level': 20, 'mp': costs[5], 'damage': 100, 'launch': '0xB5EE', 'update': '0xB1A6',
         'behaviour': 'single large blast object, sound 0x9B [partially decoded]'},
    ],
    'learn_messages': {str(k + 5): v for k, v in SPELL_MSG.items()},
}
dump('spells.json', spells)

# ---------------------------------------------------------------- enemies / object stat records
rooms = [be32(0x1CCAC + 4 * r) for r in range(48)]
starts = sorted(set(rooms))
FIELDS = ['type(+0x70)', 'p92', 'hp(+0x74)', 'exp_or_text(+0x78)', 'gold_or_drop(+0x7A)', 'p96', 'def(+0x98)',
          'flags(+0x66)', 'atk_contact(+0x68)', 'atk_alt(+0x6A)', 'p8C']
lists = {}
for b in starts:
    ents = []
    a = b
    nxt = min([s for s in starts if s > b] + [b + 4 * 64])
    while a < nxt:
        p = be32(a)
        if not 0x1C000 <= p < 0x20000: break
        w = [be16(p + 2 * i) for i in range(11)]
        e = {'index': len(ents), 'addr': h(p), 'type': w[0], 'p92': w[1], 'hp': w[2], 'exp': w[3], 'gold': w[4],
             'p96': w[5], 'def': w[6], 'flags': h(w[7], 4), 'atk_contact': w[8], 'atk_alt': w[9], 'p8C': w[10]}
        if w[7] & 0x20 and w[2] == 0:
            e['kind'] = 'npc'; e['text_id'] = w[3]
        elif w[2] and w[7] & 0x20 and w[2] in (1, 2, 3):
            e['kind'] = 'shopkeeper'
        if w[4] & 0x80 and w[4] < 0x100:
            e['drop_pickup_kind'] = w[4] & 0x0F
        ents.append(e)
        a += 4
    lists[h(b)] = {'rooms': [r for r in range(48) if rooms[r] == b], 'entries': ents}
enemies = {
    'source': {'room_table': '0x1CCAC (long per room → list of longs, index = spawn id from map spawn list 0x1C6D6 / 0x1CAFA)',
               'loader': '0x14060: copies 12 words into the object: +0x70,+0x92,+0x74,+0x78,+0x7A,+0x96,+0x98,+0x66,+0x68,+0x6A,+0x8C,+0x76 (records are 22 bytes apart; the 12th word overlaps the next record, so only 11 are listed)',
               'fields': {'type': 'object type / AI id (+0x70); 2 = gold pickup, 3.. = item pickups', 'hp': 'enemy HP (+0x74); for shopkeepers = shop type',
                          'exp': 'EXP awarded on kill (+0x78, added to killer player+0x8A, capped 0xFFFF at 0x147CA); for NPCs = dialogue text id',
                          'gold': 'on death (0x171AC) the enemy turns into a pickup: bit7 set → pickup kind = low nibble, else gold bag worth this many gold (pickup adds to player+0xCE at 0xA25E, cap 0xFFFF)',
                          'def': 'defense; melee damage = max(1, STR - def/2) (0x14452)',
                          'atk_contact': 'damage to player = clamp(atk - AC + 1, 1, 20) (0xA12E); atk_alt used for the other hit type (0xA09A)',
                          'flags': '+0x66 word; +0x67 bit2 poisonous, bit3 blockable by shield; 0x4021-style = NPC'}},
    'lists': lists,
}
dump('enemies.json', enemies)

# ---------------------------------------------------------------- RAM map
ram = {
    'player_structs': {'P1': '0xFF0DC0', 'P2': '0xFF0F40', 'size': '0x180'},
    'player_fields': {
        '+0x00': 'player number 1/2 (0 = slot unused)', '+0x6A': 'class 0 Fighter / 1 Mage (copied from 0xFF0A44/0xFF0A46)',
        '+0x6C/+0x6D/+0x6E': 'buttons held / newly pressed / released (SACBRLDU) copied each frame from 0xFF0210 (P1) / 0xFF0214 (P2)',
        '+0x7C': 'attack power latched from STR when an attack starts', '+0x8A': 'EXP (u16 binary, cap 0xFFFF)', '+0x8C': 'level (0-based, 19 = max)',
        '+0x8E': 'max HP', '+0x90': 'HP', '+0x92': 'STR', '+0x94': 'max MP', '+0x96': 'MP', '+0x98': 'A.C.', '+0x9A': 'AGL',
        '+0x9C': 'spells known (extra) / last applied spell count', '+0x9E': 'attack anim frame duration', '+0xA0': 'selected spell index',
        '+0xA2': 'walk speed 16.16 (AGL<<8 + 0x18000 / 0x16000)', '+0xA6': 'poisoned flag', '+0xA8': 'poison tick timer (120)',
        '+0xB2/+0xB4': 'HUD message queue (2 slots, message ids for table 0x2D750)', '+0xCE': 'GOLD (u16 binary, cap 0xFFFF)',
        '+0xD0': 'inn price scratch', '+0xD2': 'Medical Herbs', '+0xD4': 'Antidotes', '+0xD6': 'Bells of Life', '+0xD8': 'Elixirs',
        '+0xDC': 'EXP needed for next level (0xFFFF at max)', '+0xDE': 'dead/no-lives flag'},
    'globals': {
        '0xFF0A40': '2-player mode flag (1P/2P menu)', '0xFF0A44': 'P1 class (0 Fighter, 1 Mage)', '0xFF0A46': 'P2 class (= 1 - P1 class)',
        '0xFF0A48': 'room index', '0xFF0A4E': 'P1 name (3 bytes)', '0xFF0A51': 'P2 name (3 bytes)', '0xFF0A6C': 'pause/status window open',
        '0xFF0A70': 'stat recompute without touching HP/MP flag', '0xFF0A76': 'MP regen frame counter (Staff of the Mage)',
        '0xFF0A78': 'extra-life price (800 initial, x2 per purchase, cap 6400)', '0xFF0A7A': 'lives (start 2, max 9)',
        '0xFF0AC0': 'Fighter armor (0-3)', '0xFF0AC2': 'Fighter helm (0-2)', '0xFF0AC4': 'Fighter shield (0-3)', '0xFF0AC6': 'Fighter weapon (0-4)',
        '0xFF0AC8': 'Mage robe (0-2)', '0xFF0ACA': 'Mage wand (0-3)', '0xFF0ACC': 'Scale of Mermaid', '0xFF0ACE': 'Silk of the Worm',
        '0xFF0AD0': 'Lilliput Orchid', '0xFF0AD2': 'Silver Key', '0xFF0AD4': "Gina's Charm", '0xFF0AD6': "Abel's Charm", '0xFF0AD8': 'Magic Flute',
        '0xFF0ADA': 'Golden Key', '0xFF0B5C': 'spell hold counter', '0xFF0B5E': 'spell select state (0/1/2)', '0xFF0B60': 'active spell index',
        '0xFF0B66': 'spell active', '0xFF0B8A': 'decimal digit scratch (5 bytes, 0x2C31A)', '0xFF0BB2': 'shop customer player struct',
        '0xFF0BB6': 'shop type (1 inn 2 armory 3 apothecary)', '0xFF0BC4': 'shop cursor slot ptr', '0xFF0BCC/0xFF0BCE': 'shop cursor cell x/y',
        '0xFF0210..0xFF0213': 'P1 pad held/pressed/released/raw', '0xFF0214..': 'P2 pad'},
    'number_format': 'all stats are plain binary u16; printed via 0x2C31A (5 decimal digits to 0xFF0B8A) + 0x2C298 (leading zeros blanked)',
}
dump('ram.json', ram)

# ---------------------------------------------------------------- windows / menus
wins = []
for i in range(15):
    a = 0x2CEA2 + 8 * i
    w, hh, cx, cy = (be16(a + 2 * k) for k in range(4))
    wins.append({'id': i, 'w_tiles': w, 'h_tiles': hh, 'center_px': [cx - 0x80, cy - 0x80],
                 'top_left_px': [cx - 0x80 - (w // 2) * 8, cy - 0x80 - (hh // 2) * 8]})
menus = {
    'window_table': {'addr': '0x2CEA2', 'record': 'w width, w height (tiles), w centre x+0x80, w centre y+0x80; windows open/close with a grow/shrink animation (0x2CA6E open, 0x2CAAE close)', 'windows': wins},
    'status_window': {
        'open': 'START (new press, 0xFF0211 bit7 P1 / 0xFF0215 P2) toggles pause (0xFF0A6C) and opens both players\' status windows (0x2BC4 → 0x2D1AC); START again closes (0x2C10 → 0x2D21E)',
        'windows': 'P1 = window 13 (18x14 tiles, top-left px (8,0)), P2 = window 14 (18x14 at px (168,0)); a6 = 0xFFED8A / 0xFFEFB8',
        'layout_cells': {'name': '(2,0) 3 chars', 'LV label': '(7,0)', 'level': '(9,0) 2 digits (level+1)',
                         'labels': '(2,2) 4x6 block from 0x2D194 "GOLD","EXP ","   /","STR ","A.C.","AGL "',
                         'GOLD': '(7,2) 5 digits', 'EXP': '(7,3) 5 digits', 'NEXT': '(7,4) 5 digits (+0xDC)', 'STR': '(7,5)', 'A.C.': '(7,6)', 'AGL': '(7,7)',
                         'items': '(2,9) 14x3 names from 0x2D4FE ("MEDICAL HERB x", "ANTIDOTE     x", "ELIXIR       x")',
                         'counts': '(16,9..11) 1 digit: +0xD2, +0xD4, +0xD8'},
        'cursor': 'none – display only; game is paused while open',
        'screenshots': ['work/ui/rpg/st/frame-6bc20d28….png (Fighter L1)', 'work/ui/rpg/mage/frame-d8cd924a….png (Mage L1)'],
    },
    'no_item_or_equip_menu': 'Cadash has no inventory/equip/magic menu: consumables are auto-used, equipment is applied on purchase, spells are chosen by holding B (see spells.json)',
    'yes_no': 'dialogue code EE: two-line choice; UP selects first option (YES), DOWN second (NO), cursor sound 0xB4; B confirms; A or C = cancel (treated as NO). Same routine for 1P/2P select (0x6520)',
    'shop_grid': 'see shops.json buy_flow',
    'character_select': 'title → START; 1PLAYER/2PLAYERS (UP/DOWN, B) then FIGHTER/MAGE with LEFT/RIGHT (cursor sprite at px (0xD0,0xB8) / (0x138,0xB8)), B confirms (0x6658..0x670C); texts 134/135 describe the class',
}
dump('menus.json', menus)
print('wrote', sorted(os.listdir(OUT)))
