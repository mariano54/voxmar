import sys; sys.path.insert(0,__import__('os').path.dirname(__file__)); import flowers
out=['''// High-resolution flowers: 32×32 sprites drawn with tools/plants (shaded, lit from the top
// left, a darker rim on each shape's lower right). The texture array takes the size of its
// largest texture, so these sit beside 16×16 blocks without any renderer change.
(
    legend: {
        'a': "leaf:0", 'b': "leaf:1", 'c': "leaf:2", 'd': "leaf:3", 'e': "leaf:4", 'f': "leaf:5",
        'p': "petal:0", 'q': "petal:1", 'r': "petal:2", 's': "petal:3", 't': "petal:4",
        'x': "centre:0", 'y': "centre:1", 'z': "centre:2",
        'j': "second:0", 'k': "second:1", 'l': "second:2", 'm': "second:3", 'n': "second:4",
        'u': "tuft:0", 'v': "tuft:1", 'w': "tuft:2",
    },
    palettes: {
        "hi_leaf": ["#14300e", "#1f4614", "#2c5e1a", "#3c7822", "#52922c", "#6eaa3a"],
        "hi_red": ["#5a0a0e", "#981418", "#cc2a22", "#ec5236", "#ff8a62"],
        "hi_yellow": ["#8a5a06", "#c8900e", "#eebe1e", "#ffde48", "#fff49a"],
        "hi_blue": ["#141c6a", "#2234a8", "#3c5ad8", "#6a8cf0", "#a8c4ff"],
        "hi_white": ["#7a8088", "#a8aeb6", "#d2d6dc", "#eef0f2", "#ffffff"],
        "hi_poppy_centre": ["#140c10", "#2a1e24", "#4a3a2a"],
        "hi_gold_centre": ["#7a4a06", "#c88a12", "#f2c43a"],
        "hi_navy_centre": ["#0a0e3a", "#1c2470", "#3a46a0"],
        "hi_seed": ["#b0b4b8", "#dcdee2", "#ffffff"],
        "hi_leaf_blue": ["#16301e", "#21462a", "#2e5e38", "#3e7848", "#52925a", "#6aaa6e"],
        "hi_sage_leaf": ["#1e2e20", "#2a402c", "#38543a", "#486a48", "#5c8058", "#74966a"],
        "hi_pale_blue": ["#6c7c98", "#9cacc8", "#c8d6ee", "#e8f0fc", "#ffffff"],
        "hi_purple": ["#3e1258", "#62208a", "#8a3ab8", "#b268de", "#d8a4f6"],
        "hi_cyan": ["#0c4a7a", "#1470b0", "#2c9ce0", "#64c8f8", "#b4ecff"],
        "hi_orchid_centre": ["#0a2a4a", "#e0c040", "#fff0a0"],
        "hi_orange": ["#7a2a06", "#b8480c", "#e87018", "#ff9c3a", "#ffc87a"],
        "hi_pink": ["#8a2a5a", "#c04a84", "#e874aa", "#f8a4c8", "#ffd4e6"],
        "hi_butter": ["#9a6a04", "#d8a00c", "#f8d020", "#fff060", "#fffcc0"],
        "hi_bluebell": ["#1c2070", "#2e3aa8", "#4a5ad4", "#7a8af0", "#b4bcff"],
        "hi_aster": ["#3a2a7a", "#5a44aa", "#7e68d2", "#a690ec", "#cec0fa"],
        "hi_marigold": ["#8a2e04", "#c45208", "#ec7c12", "#ffa830", "#ffd070"],
        "hi_rust_centre": ["#3a1a06", "#6a320c", "#9a5414"],
        "hi_cosmos": ["#8a1e5a", "#c03c88", "#e46aae", "#f8a0cc", "#ffd4e8"],
        "hi_zinnia": ["#7a0a14", "#b81a20", "#e43a2a", "#ff6e4a", "#ffa880"],
        "hi_primrose": ["#9a8a2a", "#d0c04a", "#ece080", "#fbf4b4", "#ffffe4"],
        "hi_leaf_wet": ["#0e2a14", "#18401c", "#225828", "#2e7034", "#408a44", "#58a258"],
        "hi_forget": ["#1a3a9a", "#2e5ad0", "#4e86ee", "#86b4fa", "#c8e0ff"],
        "hi_wool_leaf": ["#3a4440", "#525e58", "#6c7a72", "#8a968e", "#a8b4ac", "#c8d0ca"],
        "hi_felt": ["#8a8c84", "#b4b6ae", "#d6d8d0", "#eceee6", "#fafaf4"],
        "hi_felt_centre": ["#7a6a2a", "#b0983a", "#d8c060"],
        "hi_saxi": ["#a85278", "#d07ca0", "#ecaac4", "#f8d4e2", "#ffffff"],
        "hi_desert": ["#9a3a0a", "#d0601a", "#f08c30", "#ffb860", "#ffe0a0"],
        "hi_seed_centre": ["#2a1608", "#4a2a10", "#6e4418"],
        "hi_lupine": ["#24308a", "#3a4cc0", "#5a72e0", "#8aa2f4", "#c4d2ff"],
        "hi_fireweed": ["#7a1258", "#ac2482", "#d846aa", "#f07ccc", "#ffb4e4"],
        "hi_goldenrod": ["#8a6004", "#c08a0c", "#e8b81e", "#ffdc48", "#fff090"],
        "hi_foxglove": ["#6a1460", "#9c2a8a", "#c84cb2", "#e480d2", "#f8c0ec"],
        "hi_heath_leaf": ["#1a2a1a", "#243a22", "#30502c", "#3e6636", "#507e44", "#689656"],
        "hi_heather": ["#5a1a52", "#8a2e7c", "#b44aa4", "#d878c6", "#f0b4e2"],
        "hi_thyme": ["#5a3a7a", "#7e5aa4", "#a280c8", "#c4a8e2", "#e4d4f6"],
        "hi_clover": ["#8a3a5a", "#b8607e", "#dc90a8", "#f2c4d2", "#fff0f4"],
        "hi_fern_leaf": ["#18321a", "#224a24", "#2e6230", "#3e7c3e", "#529450", "#6aac64"],
        "hi_thistle_leaf": ["#20321e", "#2e4628", "#3e5e36", "#527646", "#688e5a", "#82a872"],
        "hi_thistle": ["#4a1258", "#742486", "#a042b4", "#c874dc", "#e8b0f6"],
        "hi_iris": ["#2a1470", "#4428a4", "#6448d0", "#9078ec", "#c4b4ff"],
        "hi_crocus": ["#3a1460", "#5c2490", "#8240bc", "#aa6ede", "#d4aaf6"],
        "hi_gentian": ["#0a1880", "#1428b4", "#2444e0", "#4e72f4", "#94b4ff"],
        "hi_peony": ["#8a1446", "#c03a72", "#e46a9c", "#f8a0c4", "#ffd8e8"],
        "hi_violet": ["#24104a", "#3e1e7a", "#5c34a8", "#8058d0", "#ac8cec"],
        "hi_columbine": ["#2a2080", "#4234b4", "#6454da", "#8c80f0", "#c4bcff"],
        "hi_bell": ["#3a2484", "#5838b4", "#7a5cd8", "#a088f0", "#ccbcff"],
        "hi_leaf_dark": ["#0c2410", "#143618", "#1e4a20", "#2a602a", "#387a34", "#4c9244"],
        "hi_hibiscus": ["#7a0a1a", "#b81428", "#e02a3a", "#f85a5a", "#ff98a0"],
        "hi_lavender": ["#3a2a6a", "#5a429a", "#7c62c4", "#a28ae2", "#cabcf6"],
    },
    textures: {''']
for name,(fn,pal) in flowers.F.items():
    rows=fn().rows()
    pal=dict(pal); pal.setdefault('tuft','hi_seed'); pal.setdefault('centre',pal.get('petal')); pal.setdefault('second',pal.get('petal'))
    ps=", ".join(f'"{k}": "{v}"' for k,v in pal.items())
    out.append(f'        "block/{name}": (size: (32, 32), palettes: {{{ps}}}, layers: [Grid(rows: [')
    out+= [f'            "{r}",' for r in rows]
    out.append('        ])]),')
out.append('    },\n)\n')
open(sys.argv[1] if len(sys.argv)>1 else "assets/packs/default/recipes/flowers.ron","w").write("\n".join(out))
