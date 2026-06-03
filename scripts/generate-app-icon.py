from __future__ import annotations

import math
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter, ImageFont


ROOT = Path(__file__).resolve().parents[1]
SIZE = 1024
PREVIEW_DIR = ROOT / "docs" / "icon-previews"
TEXT = "wapp"
PREVIEW_SIZES = (1024, 128, 64, 32, 16)


def load_font(size: int) -> ImageFont.FreeTypeFont | ImageFont.ImageFont:
    for name in ("arialbd.ttf", "Arial Bold.ttf", "segoeuib.ttf", "DejaVuSans-Bold.ttf"):
        try:
            return ImageFont.truetype(name, size)
        except OSError:
            continue
    return ImageFont.load_default()


def ellipse_layer(size: int, box: list[int], fill: tuple[int, int, int, int]) -> Image.Image:
    layer = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    ImageDraw.Draw(layer).ellipse(box, fill=fill)
    return layer


def draw_centered_text(
    draw: ImageDraw.ImageDraw,
    size: int,
    text: str,
    font_size: int,
    y_offset: int,
    fill: tuple[int, int, int, int],
    shadow: tuple[int, int, int, tuple[int, int, int, int]] | None = None,
) -> None:
    font = load_font(font_size)
    text_box = draw.textbbox((0, 0), text, font=font)
    text_width = text_box[2] - text_box[0]
    text_height = text_box[3] - text_box[1]
    x = (size - text_width) / 2
    y = (size - text_height) / 2 + y_offset

    if shadow:
        dx, dy, blur, color = shadow
        shadow_layer = Image.new("RGBA", (size, size), (0, 0, 0, 0))
        shadow_draw = ImageDraw.Draw(shadow_layer)
        shadow_draw.text((x + dx, y + dy), text, font=font, fill=color)
        if blur > 0:
            shadow_layer = shadow_layer.filter(ImageFilter.GaussianBlur(blur))
        draw._image.alpha_composite(shadow_layer)

    draw.text((x, y), text, font=font, fill=fill)


def rounded_rect_layer(size: int, radius: int, fill: tuple[int, int, int, int]) -> Image.Image:
    layer = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    ImageDraw.Draw(layer).rounded_rectangle([0, 0, size, size], radius=radius, fill=fill)
    return layer


def composite_with_mask(base: Image.Image, layer: Image.Image, mask: Image.Image) -> None:
    clipped = Image.composite(layer, Image.new("RGBA", layer.size, (0, 0, 0, 0)), mask)
    base.alpha_composite(clipped)


def draw_ios_background(draw: ImageDraw.ImageDraw, image: Image.Image, variant: str) -> None:
    radius = 210
    if variant == "glossy-beat":
        gradient = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
        gradient_draw = ImageDraw.Draw(gradient)
        for y in range(SIZE):
            t = y / (SIZE - 1)
            color = (
                int(34 + t * 15),
                int(37 + t * 14),
                int(44 + t * 18),
                255,
            )
            gradient_draw.line((0, y, SIZE, y), fill=color)
        mask = rounded_rect_layer(SIZE, radius, (255, 255, 255, 255)).split()[-1]
        composite_with_mask(image, gradient, mask)
        top_glow = ellipse_layer(SIZE, [88, -160, 936, 520], (255, 255, 255, 24)).filter(ImageFilter.GaussianBlur(34))
        composite_with_mask(image, top_glow, mask)
    elif variant == "rhythm-pulse":
        draw.rounded_rectangle([0, 0, SIZE, SIZE], radius=radius, fill=(31, 34, 41, 255))
        center = SIZE / 2
        for index in range(54):
            angle = math.tau * index / 54
            start = 260
            end = 470 + (index % 6) * 9
            x1 = center + math.cos(angle) * start
            y1 = center + math.sin(angle) * start
            x2 = center + math.cos(angle) * end
            y2 = center + math.sin(angle) * end
            draw.line((x1, y1, x2, y2), fill=(255, 107, 186, 26), width=4)
    else:
        draw.rounded_rectangle([0, 0, SIZE, SIZE], radius=radius, fill=(35, 38, 45, 255))
        draw.rounded_rectangle([26, 26, SIZE - 26, SIZE - 26], radius=184, outline=(255, 255, 255, 20), width=3)


def draw_rings(
    draw: ImageDraw.ImageDraw,
    size: int,
    margin: int,
    ring_color: tuple[int, int, int, int],
    inner_color: tuple[int, int, int, int],
) -> None:
    draw.ellipse(
        [margin + 20, margin + 20, size - margin - 20, size - margin - 20],
        outline=ring_color,
        width=54,
    )
    draw.ellipse(
        [margin + 88, margin + 88, size - margin - 88, size - margin - 88],
        outline=inner_color,
        width=18,
    )


def make_classic_plus() -> Image.Image:
    image = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)
    draw_ios_background(draw, image, "classic-plus")
    margin = 162

    image.alpha_composite(ellipse_layer(SIZE, [margin + 20, margin + 34, SIZE - margin + 20, SIZE - margin + 34], (10, 10, 14, 110)).filter(ImageFilter.GaussianBlur(18)))
    draw.ellipse([margin, margin, SIZE - margin, SIZE - margin], fill=(235, 55, 149, 255))
    draw.ellipse([margin + 86, margin + 92, SIZE - margin - 86, SIZE - margin - 74], fill=(225, 42, 137, 255))
    draw_rings(draw, SIZE, margin, (255, 180, 220, 255), (178, 26, 116, 150))

    sheen = ellipse_layer(SIZE, [238, 188, 786, 394], (255, 255, 255, 54)).filter(ImageFilter.GaussianBlur(18))
    image.alpha_composite(sheen)
    draw_centered_text(draw, SIZE, TEXT, 174, -18, (255, 255, 255, 255), (7, 10, 3, (91, 8, 70, 132)))
    return image


def make_glossy_beat() -> Image.Image:
    image = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)
    draw_ios_background(draw, image, "glossy-beat")
    margin = 150

    logo_box = [margin, margin, SIZE - margin, SIZE - margin]
    logo_mask = ellipse_layer(SIZE, logo_box, (255, 255, 255, 255)).split()[-1]
    recess = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    recess_draw = ImageDraw.Draw(recess)
    recess_draw.ellipse([margin - 18, margin - 18, SIZE - margin + 18, SIZE - margin + 18], fill=(10, 12, 18, 150))
    recess_draw.ellipse([margin + 10, margin + 14, SIZE - margin - 10, SIZE - margin - 4], fill=(255, 255, 255, 22))
    image.alpha_composite(recess.filter(ImageFilter.GaussianBlur(8)))

    cut_shadow = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    cut_shadow_draw = ImageDraw.Draw(cut_shadow)
    cut_shadow_draw.arc([margin - 4, margin - 8, SIZE - margin + 4, SIZE - margin + 8], start=186, end=356, fill=(6, 7, 11, 164), width=34)
    image.alpha_composite(cut_shadow.filter(ImageFilter.GaussianBlur(3)))

    for index in range(44):
        t = index / 43
        inset = int(margin + t * 90)
        color = (
            int(246 - t * 42),
            int(76 - t * 38),
            int(164 - t * 32),
            255,
        )
        draw.ellipse([inset, inset, SIZE - inset, SIZE - inset], fill=color)

    inset_shadow = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    inset_draw = ImageDraw.Draw(inset_shadow)
    inset_draw.ellipse([margin + 12, margin + 4, SIZE - margin - 12, SIZE - margin - 18], outline=(74, 5, 58, 128), width=44)
    inset_shadow = inset_shadow.filter(ImageFilter.GaussianBlur(4))
    composite_with_mask(image, inset_shadow, logo_mask)

    inset_light = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    inset_light_draw = ImageDraw.Draw(inset_light)
    inset_light_draw.arc([margin + 24, margin + 18, SIZE - margin - 24, SIZE - margin - 30], start=198, end=344, fill=(255, 238, 248, 130), width=20)
    composite_with_mask(image, inset_light, logo_mask)

    draw.ellipse([margin + 16, margin + 16, SIZE - margin - 16, SIZE - margin - 16], outline=(255, 207, 234, 188), width=30)
    draw.ellipse([margin + 88, margin + 88, SIZE - margin - 88, SIZE - margin - 88], outline=(126, 19, 104, 134), width=22)
    highlight = ellipse_layer(SIZE, [230, 142, 794, 360], (255, 255, 255, 90)).filter(ImageFilter.GaussianBlur(26))
    composite_with_mask(image, highlight, logo_mask)
    draw.arc([238, 218, 786, 786], start=214, end=334, fill=(120, 8, 89, 110), width=16)
    draw_centered_text(draw, SIZE, TEXT, 184, -18, (255, 255, 255, 245), (5, 7, 2, (70, 2, 57, 120)))
    return image


def make_glossy_logo() -> Image.Image:
    image = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)
    margin = 150
    logo_box = [margin, margin, SIZE - margin, SIZE - margin]
    logo_mask = ellipse_layer(SIZE, logo_box, (255, 255, 255, 255)).split()[-1]

    shadow = ellipse_layer(SIZE, [margin + 20, margin + 34, SIZE - margin + 20, SIZE - margin + 34], (8, 8, 12, 120)).filter(ImageFilter.GaussianBlur(22))
    image.alpha_composite(shadow)

    for index in range(44):
        t = index / 43
        inset = int(margin + t * 90)
        color = (
            int(246 - t * 42),
            int(76 - t * 38),
            int(164 - t * 32),
            255,
        )
        draw.ellipse([inset, inset, SIZE - inset, SIZE - inset], fill=color)

    inset_shadow = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    inset_draw = ImageDraw.Draw(inset_shadow)
    inset_draw.ellipse([margin + 12, margin + 4, SIZE - margin - 12, SIZE - margin - 18], outline=(74, 5, 58, 128), width=44)
    composite_with_mask(image, inset_shadow.filter(ImageFilter.GaussianBlur(4)), logo_mask)

    draw.ellipse([margin + 16, margin + 16, SIZE - margin - 16, SIZE - margin - 16], outline=(255, 207, 234, 188), width=30)
    draw.ellipse([margin + 88, margin + 88, SIZE - margin - 88, SIZE - margin - 88], outline=(126, 19, 104, 134), width=22)
    highlight = ellipse_layer(SIZE, [230, 142, 794, 360], (255, 255, 255, 90)).filter(ImageFilter.GaussianBlur(26))
    composite_with_mask(image, highlight, logo_mask)
    draw_centered_text(draw, SIZE, TEXT, 184, -18, (255, 255, 255, 245), (5, 7, 2, (70, 2, 57, 120)))
    return image


def make_rhythm_pulse() -> Image.Image:
    image = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)
    draw_ios_background(draw, image, "rhythm-pulse")
    margin = 158
    center = SIZE / 2
    radius = SIZE / 2 - margin - 54

    image.alpha_composite(ellipse_layer(SIZE, [margin + 20, margin + 30, SIZE - margin + 20, SIZE - margin + 30], (8, 8, 12, 118)).filter(ImageFilter.GaussianBlur(18)))
    draw.ellipse([margin, margin, SIZE - margin, SIZE - margin], fill=(232, 49, 145, 255))
    draw_rings(draw, SIZE, margin, (255, 185, 224, 255), (166, 23, 112, 145))

    for index in range(40):
        angle = (math.tau / 40) * index
        pulse = 20 + (index % 5) * 8
        start = radius - 28
        end = radius + pulse
        x1 = center + math.cos(angle) * start
        y1 = center + math.sin(angle) * start
        x2 = center + math.cos(angle) * end
        y2 = center + math.sin(angle) * end
        alpha = 62 if index % 5 == 0 else 34
        draw.line((x1, y1, x2, y2), fill=(255, 222, 242, alpha), width=5)

    draw.ellipse([294, 294, 730, 730], fill=(234, 58, 151, 235))
    draw.ellipse([300, 300, 724, 724], outline=(255, 210, 235, 86), width=9)
    draw_centered_text(draw, SIZE, TEXT, 170, -16, (255, 255, 255, 255), (7, 10, 2, (80, 4, 64, 138)))
    return image


def resize_icon(image: Image.Image, size: int) -> Image.Image:
    return image.resize((size, size), Image.Resampling.LANCZOS)


def make_preview_sheet(icons: dict[str, Image.Image]) -> Image.Image:
    label_font = load_font(28)
    cell_width = 220
    left_width = 250
    top_height = 72
    row_height = 178
    sheet = Image.new(
        "RGBA",
        (left_width + cell_width * len(PREVIEW_SIZES), top_height + row_height * len(icons)),
        (245, 243, 238, 255),
    )
    draw = ImageDraw.Draw(sheet)

    for column, size in enumerate(PREVIEW_SIZES):
        draw.text((left_width + column * cell_width + 20, 24), f"{size}px", font=label_font, fill=(48, 55, 67, 255))

    for row, (name, icon) in enumerate(icons.items()):
        y = top_height + row * row_height
        draw.text((26, y + 72), name, font=label_font, fill=(48, 55, 67, 255))
        for column, preview_size in enumerate(PREVIEW_SIZES):
            display_size = min(preview_size, 128)
            tile_x = left_width + column * cell_width
            draw.rounded_rectangle([tile_x + 12, y + 14, tile_x + cell_width - 12, y + row_height - 14], radius=10, fill=(255, 255, 255, 255), outline=(224, 220, 211, 255))
            rendered = resize_icon(icon, display_size)
            x = tile_x + (cell_width - display_size) // 2
            icon_y = y + (row_height - display_size) // 2
            sheet.alpha_composite(rendered, (x, icon_y))

    return sheet


def build_variants() -> dict[str, Image.Image]:
    return {
        "classic-plus": make_classic_plus(),
        "glossy-beat": make_glossy_beat(),
        "rhythm-pulse": make_rhythm_pulse(),
    }


def main() -> None:
    PREVIEW_DIR.mkdir(parents=True, exist_ok=True)
    icons = build_variants()

    for name, icon in icons.items():
        icon.save(PREVIEW_DIR / f"{name}.png")
        for preview_size in PREVIEW_SIZES[1:]:
            resize_icon(icon, preview_size).save(PREVIEW_DIR / f"{name}-{preview_size}.png")

    make_preview_sheet(icons).save(PREVIEW_DIR / "comparison.png")
    make_glossy_logo().save(PREVIEW_DIR / "glossy-logo.png")


if __name__ == "__main__":
    main()
