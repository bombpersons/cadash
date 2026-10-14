"""Human names for object types (type table, type id) and type-table groups.
Identified from rendered sprites, NPC dialogue ids (record .exp = text id, see extract/out/text/strings.json)
and boss messages.  Unnamed types keep 'typeN'.  See docs/re/objects.md."""

GROUP_NAMES = {
    0x1F5BE: 'castle_town',      # rooms 0,1,4,5,12,18,23,29-37,39-43,45,47: Dirzar castle/town, grotto, Marinade, gnome forest/hideout, houses+shops, ending
    0x1F1DA: 'caves',            # rooms 2,28: first cave (orcs, skeletons, spiders, rolling eyes)
    0x1EF86: 'black_pudding',    # room 3: first boss
    0x1F25A: 'lake_caves',       # rooms 6,7,8,10,38 (Kraken area)
    0x1EFEE: 'kraken',           # rooms 9,11: mermaid spring / Kraken boss
    0x1F432: 'fire_halls',       # rooms 13,22
    0x1F2DA: 'forest',           # rooms 14,15,16: trackless forest / worm grotto approach
    0x1F04E: 'worm',             # room 17: Worm boss
    0x1F366: 'ruined_land',      # rooms 19,20: destroyed country, bandits, Alex the dog
    0x1F47E: 'sea_of_flame',     # room 21
    0x1F0B6: 'fire_elemental',   # room 24: Fire Elemental boss
    0x1F4C6: 'balrog_castle',    # rooms 25,26,27: Balrog's castle, Jelly Giant, worm re-fight, princess
    0x1F13E: 'balrog',           # rooms 44,46: Balrog final battles
}

_C = 0x1F5BE
NAMES = {
    # ---- castle / town / villages (group castle_town)
    (_C, 16): 'villager_porter', (_C, 17): 'king', (_C, 18): 'lady_white', (_C, 19): 'priest_blue',
    (_C, 20): 'townsman_walker', (_C, 21): 'gnome_a', (_C, 22): 'gnome_b', (_C, 23): 'horse',
    (_C, 24): 'townsman_walker_b', (_C, 25): 'shopkeeper_old_man', (_C, 26): 'shopkeeper_woman', (_C, 27): 'old_woman',
    (_C, 28): 'soldier', (_C, 29): 'gnome_c', (_C, 30): 'lady_white_b', (_C, 31): 'priest_robed',
    (_C, 32): 'beast_mount', (_C, 33): 'palace_guard', (_C, 34): 'gnome_d', (_C, 35): 'gnome_e', (_C, 36): 'gnome_f',
    (_C, 37): 'gnome_g', (_C, 38): 'locked_door', (_C, 39): 'gina', (_C, 40): 'dying_soldier',
    (_C, 41): 'princess_salassa', (_C, 42): 'princess_salassa_b',
    # ---- first caves
    (0x1F1DA, 14): 'hidden_item_spot', (0x1F1DA, 16): 'orc_flail', (0x1F1DA, 17): 'bat', (0x1F1DA, 18): 'rock_platform_slow',
    (0x1F1DA, 19): 'rock_platform', (0x1F1DA, 20): 'rock_spout', (0x1F1DA, 21): 'spout_rock_up', (0x1F1DA, 22): 'spout_rock_right',
    (0x1F1DA, 23): 'spout_rock_left', (0x1F1DA, 24): 'skeleton_shield', (0x1F1DA, 25): 'rolling_eye',
    (0x1F1DA, 26): 'caterpillar', (0x1F1DA, 27): 'spider', (0x1F1DA, 28): 'spider_web_right', (0x1F1DA, 29): 'spider_web_left',
    (0x1F1DA, 30): 'rolling_eye_b', (0x1F1DA, 31): 'frightened_man',
    # ---- Black Pudding
    (0x1EF86, 13): 'boss_death', (0x1EF86, 16): 'black_pudding', (0x1EF86, 17): 'pudding_blob',
    **{(0x1EF86, t): 'pudding_debris_%d' % (t - 17) for t in range(18, 26)},
    # ---- lake caves
    (0x1F25A, 14): 'hidden_item_spot', (0x1F25A, 16): 'tree_monster', (0x1F25A, 18): 'blue_troll', (0x1F25A, 21): 'hornet',
    (0x1F25A, 22): 'piranha_head', (0x1F25A, 25): 'frog', (0x1F25A, 31): 'mermaid_girl',
    # ---- Kraken
    (0x1EFEE, 16): 'kraken', (0x1EFEE, 21): 'girl_guardian', (0x1EFEE, 22): 'mermaid', (0x1EFEE, 23): 'spring_sparkle',
    # ---- forest
    (0x1F2DA, 14): 'hidden_item_spot', (0x1F2DA, 16): 'stone_golem', (0x1F2DA, 18): 'hedgehog', (0x1F2DA, 19): 'harpy',
    (0x1F2DA, 23): 'carnivorous_plant', (0x1F2DA, 27): 'piranha_head', (0x1F2DA, 32): 'spider_brown',
    # ---- Worm
    (0x1F04E, 16): 'worm_head', **{(0x1F04E, t): 'worm_segment_%d' % (t - 16) for t in range(17, 24)},
    # ---- ruined land
    (0x1F366, 16): 'trap_block', (0x1F366, 17): 'trap_block_b', (0x1F366, 24): 'green_dragon', (0x1F366, 27): 'mummy',
    (0x1F366, 28): 'mummy_b', (0x1F366, 29): 'mummy_c', (0x1F366, 30): 'grim_reaper', (0x1F366, 46): 'ghost_villager',
    (0x1F366, 47): 'alex_dog', (0x1F366, 48): 'alex_dog_b', (0x1F366, 49): 'alex_dog_running', (0x1F366, 50): 'bandit',
    # ---- fire elemental
    (0x1F0B6, 13): 'boss_death', (0x1F0B6, 16): 'fire_elemental', (0x1F0B6, 33): 'bandit_chief',
    # ---- Balrog castle
    (0x1F4C6, 28): 'blue_dragon', (0x1F4C6, 31): 'skeleton_shield', (0x1F4C6, 38): 'locked_door', (0x1F4C6, 39): 'princess_salassa',
    (0x1F4C6, 40): 'jelly_giant', (0x1F4C6, 51): 'worm_head', (0x1F4C6, 61): 'balrog_disguised',
    # ---- Balrog
    (0x1F13E, 16): 'balrog', (0x1F13E, 29): 'balrog_ending',
}
