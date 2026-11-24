#!/usr/bin/env python3
"""Block 2 reference files made by independent producers (python-docx, openpyxl,
odfpy, LibreOffice, reportlab, Pillow, ffmpeg). Usage: make_fixtures.py <dir>.
Run with -I from a venv that has: python-docx openpyxl odfpy reportlab pillow.
LibreOffice (soffice) and ffmpeg are optional; missing ones are reported.
Test: PARANOIC_BLOCK2_FIXTURES=<dir> cargo test -p paranoic-documents --test block2_fixtures
"""
import datetime, os, shutil, subprocess, sys

out = sys.argv[1]
os.makedirs(out, exist_ok=True)
p = lambda name: os.path.join(out, name)

# --- DOCX (python-docx) ---
import docx
d = docx.Document()
d.add_heading("Отчёт Paranoic", level=0)
d.add_heading("Раздел один", level=1)
para = d.add_paragraph("Обычный ")
para.add_run("жирный").bold = True
para.add_run(" и ")
para.add_run("курсив").italic = True
para.add_run(" и ")
para.add_run("подчёркнутый").underline = True
d.add_paragraph("пункт списка", style="List Bullet")
table = d.add_table(rows=2, cols=2)
for r, row in enumerate([["A1", "B1"], ["A2", "Б2"]]):
    for c, text in enumerate(row):
        table.cell(r, c).text = text
d.save(p("python.docx"))

# --- XLSX (openpyxl), formula without cached value ---
import openpyxl
wb = openpyxl.Workbook()
ws = wb.active
ws.title = "Данные"
ws.append(["Имя", "Число", "Дата"])
ws.append(["альфа", 42, datetime.date(2024, 2, 29)])
ws.append(["бета", 3.5, datetime.date(1999, 12, 31)])
ws["E10"] = "далеко"
ws["B4"] = "=B2*2"
wb.create_sheet("Второй")["A1"] = "второй лист"
wb.save(p("openpyxl.xlsx"))

# --- ODS (odfpy) ---
from odf.opendocument import OpenDocumentSpreadsheet
from odf.table import Table, TableRow, TableCell
from odf.text import P
ods = OpenDocumentSpreadsheet()
t = Table(name="Лист1")
for values in [["x", "1"], ["y", "2"]]:
    tr = TableRow()
    for v in values:
        if v.isdigit():
            cell = TableCell(valuetype="float", value=v)
        else:
            cell = TableCell(valuetype="string")
        cell.addElement(P(text=v))
        tr.addElement(cell)
    t.addElement(tr)
# A row repeated a million times must not allocate a million rows.
tr = TableRow(numberrowsrepeated=1000000)
tr.addElement(TableCell(numbercolumnsrepeated=16384))
t.addElement(tr)
ods.spreadsheet.addElement(t)
ods.save(p("odfpy.ods"))

# --- LibreOffice conversions (recalculated cached values) ---
soffice = shutil.which("soffice")
if soffice:
    for source, target in [("openpyxl.xlsx", "ods"), ("openpyxl.xlsx", "xlsx"), ("python.docx", "docx")]:
        work = p("lo-" + target)
        os.makedirs(work, exist_ok=True)
        subprocess.run([soffice, "--headless", "--convert-to", target, "--outdir", work, p(source)],
                       check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=300)
        produced = os.path.join(work, os.path.splitext(source)[0] + "." + target)
        os.replace(produced, p("libreoffice." + target))
        shutil.rmtree(work)
else:
    print("soffice missing: LibreOffice fixtures skipped")

# --- PDF (reportlab), 3 pages ---
from reportlab.pdfgen import canvas
c = canvas.Canvas(p("three-pages.pdf"))
for word in ["alpha first page", "beta second page", "alpha third page"]:
    c.drawString(72, 720, word)
    c.showPage()
c.save()

# --- Images (Pillow) ---
from PIL import Image
img = Image.new("RGB", (300, 120), (200, 30, 30))
for x in range(0, 300, 10):
    img.putpixel((x, 60), (0, 0, 255))
for fmt, ext in [("PNG", "png"), ("JPEG", "jpg"), ("GIF", "gif"), ("WEBP", "webp"), ("BMP", "bmp")]:
    img.save(p("image." + ext), fmt)
# EXIF orientation 6 (rotate 90): stored 300x120, displayed 120x300.
exif = Image.Exif()
exif[0x0112] = 6
img.save(p("rotated.jpg"), "JPEG", exif=exif.tobytes())
# Dimension bomb: tiny file declaring 60000x60000 pixels.
Image.new("1", (60000, 60000)).save(p("bomb.png"), "PNG", optimize=True)

# --- Audio/video (ffmpeg) ---
ffmpeg = shutil.which("ffmpeg")
if ffmpeg:
    def ff(args, name):
        r = subprocess.run([ffmpeg, "-y", "-loglevel", "error", *args, p(name)], timeout=120)
        if r.returncode != 0:
            print(f"ffmpeg could not produce {name}")
    tone = ["-f", "lavfi", "-i", "sine=frequency=440:duration=1"]
    video = ["-f", "lavfi", "-i", "testsrc=duration=1:size=64x48:rate=5"]
    ff(tone, "audio.wav")
    ff(tone + ["-c:a", "flac"], "audio.flac")
    ff(tone + ["-c:a", "libvorbis"], "audio.ogg")
    ff(tone + ["-c:a", "libmp3lame"], "audio.mp3")
    ff(tone + ["-c:a", "aac", "-f", "ipod"], "audio.m4a")
    ff(video + ["-c:v", "libvpx", "-f", "webm"], "video.webm")
    ff(video + ["-c:v", "mpeg4", "-f", "mp4"], "video.mp4")
    ff(video + ["-c:v", "mpeg4", "-f", "matroska"], "video.mkv")
else:
    print("ffmpeg missing: media fixtures skipped")
print("fixtures in", out)
