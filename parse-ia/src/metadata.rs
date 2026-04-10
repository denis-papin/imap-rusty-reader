use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use html_escape::encode_safe;
use img_parts::jpeg::{Jpeg, JpegSegment, markers};
use img_parts::png::{Png, PngChunk};
use img_parts::riff::{RiffChunk, RiffContent};
use img_parts::webp::{CHUNK_VP8, CHUNK_VP8L, CHUNK_VP8X, CHUNK_XMP, WebP};
use lofty::config::WriteOptions;
use lofty::file::{FileType, TaggedFileExt};
use lofty::id3::v2::Id3v2Tag;
use lofty::ogg::VorbisComments;
use lofty::prelude::AudioFile;
use lofty::probe::Probe;
use log::{info, warn};
use lopdf::{Document, Stream, dictionary};
use regex::Regex;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

const DOKA_NAMESPACE_URL: &str = "https://wks.tools/ns/doka/1.0/";
const DOKA_NAMESPACE_PREFIX: &str = "doka";
const DOKA_PROPERTY_NAME: &str = "doka-custom";
const JPEG_XMP_PREFIX: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
const PNG_ITXT_CHUNK: [u8; 4] = [b'i', b'T', b'X', b't'];
const WEBP_XMP_FLAG: u8 = 0b0000_0100;
const BMFF_XMP_UUID: [u8; 16] = [
    0xBE, 0x7A, 0xCF, 0xCB, 0x97, 0xA9, 0x42, 0xE8, 0x9C, 0x71, 0x99, 0x94, 0x91, 0xE3, 0xAF, 0xAC,
];

pub fn embed_custom_metadata(path: &Path, payload: &str) -> Result<bool> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase());

    match extension.as_deref() {
        Some("pdf") => {
            write_pdf_xmp(path, payload)?;
            Ok(true)
        }
        Some("jpg") | Some("jpeg") => {
            write_jpeg_xmp(path, payload)?;
            Ok(true)
        }
        Some("png") => {
            write_png_xmp(path, payload)?;
            Ok(true)
        }
        Some("webp") => {
            write_webp_xmp(path, payload)?;
            Ok(true)
        }
        Some("mp3") | Some("aac") | Some("ogg") | Some("oga") | Some("opus") | Some("flac") => {
            write_audio_metadata(path, payload)?;
            Ok(true)
        }
        Some("svg") => {
            write_svg_xmp(path, payload)?;
            Ok(true)
        }
        Some("mp4") | Some("mov") | Some("m4v") | Some("3gp") | Some("3g2") | Some("avif")
        | Some("heic") | Some("heif") | Some("m4a") => {
            write_bmff_xmp(path, payload)?;
            Ok(true)
        }
        Some("mkv") | Some("webm") | Some("avi") | Some("wmv") | Some("mpg") | Some("mpeg")
        | Some("ts") | Some("m2ts") => {
            write_ffmpeg_metadata(path, payload)?;
            Ok(true)
        }
        Some("docx") | Some("docm") | Some("dotx") | Some("dotm") | Some("xlsx") | Some("xlsm")
        | Some("xltx") | Some("xltm") | Some("pptx") | Some("pptm") | Some("potx")
        | Some("potm") | Some("ppsx") | Some("ppsm") => {
            write_openxml_custom_property(path, payload)?;
            Ok(true)
        }
        Some("epub") => {
            write_epub_custom_metadata(path, payload)?;
            Ok(true)
        }
        Some("odt") | Some("ods") | Some("odp") | Some("odg") | Some("ott") | Some("ots")
        | Some("otp") | Some("otg") => {
            write_opendocument_custom_property(path, payload)?;
            Ok(true)
        }
        Some("rtf") => {
            write_rtf_custom_metadata(path, payload)?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

fn build_xmp_packet(payload: &str) -> String {
    let escaped_payload = encode_safe(payload);
    format!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\
<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\
<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
<rdf:Description rdf:about=\"\" xmlns:{prefix}=\"{url}\">\
<{prefix}:{name}>{payload}</{prefix}:{name}>\
</rdf:Description>\
</rdf:RDF>\
</x:xmpmeta><?xpacket end=\"r\"?>",
        prefix = DOKA_NAMESPACE_PREFIX,
        url = DOKA_NAMESPACE_URL,
        name = DOKA_PROPERTY_NAME,
        payload = escaped_payload,
    )
}

fn write_pdf_xmp(path: &Path, payload: &str) -> Result<()> {
    let mut document =
        Document::load(path).with_context(|| format!("unable to open pdf {}", path.display()))?;
    let metadata = Stream::new(
        dictionary! {
            "Type" => "Metadata",
            "Subtype" => "XML",
        },
        build_xmp_packet(payload).into_bytes(),
    );
    let metadata_id = document.add_object(metadata);
    document
        .catalog_mut()
        .with_context(|| format!("unable to access catalog for {}", path.display()))?
        .set("Metadata", metadata_id);

    let mut output = Vec::new();
    document
        .save_to(&mut output)
        .with_context(|| format!("unable to save pdf {}", path.display()))?;
    fs::write(path, output).with_context(|| format!("unable to rewrite {}", path.display()))?;
    Ok(())
}

fn write_jpeg_xmp(path: &Path, payload: &str) -> Result<()> {
    let input = fs::read(path).with_context(|| format!("unable to read {}", path.display()))?;
    let mut jpeg = Jpeg::from_bytes(input.into())
        .with_context(|| format!("unable to parse jpeg {}", path.display()))?;

    jpeg.segments_mut().retain(|segment| {
        !(segment.marker() == markers::APP1 && segment.contents().starts_with(JPEG_XMP_PREFIX))
    });

    let mut contents = Vec::with_capacity(JPEG_XMP_PREFIX.len() + payload.len());
    contents.extend_from_slice(JPEG_XMP_PREFIX);
    contents.extend_from_slice(build_xmp_packet(payload).as_bytes());

    let segment = JpegSegment::new_with_contents(markers::APP1, contents.into());
    let insert_at = jpeg
        .segments()
        .iter()
        .position(|segment| segment.marker() != markers::APP0 && segment.marker() != markers::APP1)
        .unwrap_or(jpeg.segments().len());
    jpeg.segments_mut().insert(insert_at, segment);

    let mut output = Vec::new();
    jpeg.encoder()
        .write_to(&mut output)
        .with_context(|| format!("unable to write jpeg {}", path.display()))?;
    fs::write(path, output).with_context(|| format!("unable to rewrite {}", path.display()))?;
    Ok(())
}

fn write_png_xmp(path: &Path, payload: &str) -> Result<()> {
    let input = fs::read(path).with_context(|| format!("unable to read {}", path.display()))?;
    let mut png = Png::from_bytes(input.into())
        .with_context(|| format!("unable to parse png {}", path.display()))?;

    png.chunks_mut().retain(|chunk| {
        !(chunk.kind() == PNG_ITXT_CHUNK && is_png_xmp_chunk(chunk.contents().as_ref()))
    });

    let chunk = PngChunk::new(
        PNG_ITXT_CHUNK,
        build_png_xmp_chunk(build_xmp_packet(payload)),
    );
    let insert_at = png.chunks().len().saturating_sub(1);
    png.chunks_mut().insert(insert_at, chunk);

    let mut output = Vec::new();
    png.encoder()
        .write_to(&mut output)
        .with_context(|| format!("unable to write png {}", path.display()))?;
    fs::write(path, output).with_context(|| format!("unable to rewrite {}", path.display()))?;
    Ok(())
}

fn is_png_xmp_chunk(contents: &[u8]) -> bool {
    contents.starts_with(b"XML:com.adobe.xmp\0")
}

fn build_png_xmp_chunk(packet: String) -> img_parts::Bytes {
    let mut contents = Vec::with_capacity(22 + packet.len());
    contents.extend_from_slice(b"XML:com.adobe.xmp\0");
    contents.push(0);
    contents.push(0);
    contents.push(0);
    contents.push(0);
    contents.extend_from_slice(packet.as_bytes());
    contents.into()
}

fn write_webp_xmp(path: &Path, payload: &str) -> Result<()> {
    let input = fs::read(path).with_context(|| format!("unable to read {}", path.display()))?;
    let mut webp = WebP::from_bytes(input.into())
        .with_context(|| format!("unable to parse webp {}", path.display()))?;

    webp.remove_chunks_by_id(CHUNK_XMP);
    ensure_webp_vp8x_chunk(&mut webp)?;

    let xmp_chunk = RiffChunk::new(
        CHUNK_XMP,
        RiffContent::Data(build_xmp_packet(payload).into()),
    );
    webp.chunks_mut().push(xmp_chunk);

    let mut output = Vec::new();
    webp.encoder()
        .write_to(&mut output)
        .with_context(|| format!("unable to write webp {}", path.display()))?;
    fs::write(path, output).with_context(|| format!("unable to rewrite {}", path.display()))?;
    Ok(())
}

fn write_svg_xmp(path: &Path, payload: &str) -> Result<()> {
    let svg =
        fs::read_to_string(path).with_context(|| format!("unable to read {}", path.display()))?;
    let doka_metadata_regex =
        Regex::new(r#"(?s)\s*<metadata\b[^>]*\bid="doka-xmp"[^>]*>.*?</metadata>"#)?;
    let cleaned = doka_metadata_regex.replace_all(&svg, "").into_owned();
    let metadata_block = format!(
        "\n<metadata id=\"doka-xmp\">\n{}\n</metadata>",
        build_xmp_packet(payload)
    );

    let updated = if let Some(index) = cleaned.find('>') {
        let (head, tail) = cleaned.split_at(index + 1);
        format!("{head}{metadata_block}{tail}")
    } else {
        anyhow::bail!("invalid svg root in {}", path.display());
    };

    fs::write(path, updated).with_context(|| format!("unable to rewrite {}", path.display()))?;
    Ok(())
}

fn write_audio_metadata(path: &Path, payload: &str) -> Result<()> {
    let mut tagged_file = Probe::open(path)
        .with_context(|| format!("unable to open audio {}", path.display()))?
        .guess_file_type()
        .with_context(|| format!("unable to detect audio type {}", path.display()))?
        .read()
        .with_context(|| format!("unable to read audio metadata {}", path.display()))?;

    match tagged_file.file_type() {
        FileType::Mpeg | FileType::Aac => {
            let mut tag = tagged_file
                .tag(lofty::tag::TagType::Id3v2)
                .cloned()
                .map(Id3v2Tag::from)
                .unwrap_or_else(Id3v2Tag::new);
            tag.insert_user_text(DOKA_PROPERTY_NAME.to_string(), payload.to_string());
            tagged_file.insert_tag(tag.into());
        }
        FileType::Flac | FileType::Vorbis | FileType::Opus | FileType::Speex => {
            let mut tag = tagged_file
                .tag(lofty::tag::TagType::VorbisComments)
                .cloned()
                .map(VorbisComments::from)
                .unwrap_or_else(VorbisComments::new);
            tag.insert(DOKA_PROPERTY_NAME.to_ascii_uppercase(), payload.to_string());
            tagged_file.insert_tag(tag.into());
        }
        other => anyhow::bail!("unsupported audio metadata writer for {other:?}"),
    }

    tagged_file
        .save_to_path(path, WriteOptions::default())
        .with_context(|| format!("unable to save audio metadata {}", path.display()))?;
    Ok(())
}

fn write_bmff_xmp(path: &Path, payload: &str) -> Result<()> {
    let input = fs::read(path).with_context(|| format!("unable to read {}", path.display()))?;
    let ranges = parse_bmff_top_level_boxes(&input)?;
    let mut output = Vec::with_capacity(input.len() + payload.len() + 64);
    let xmp_box = build_bmff_xmp_box(payload)?;

    let insert_after = ranges
        .iter()
        .find(|range| &range.kind == b"ftyp")
        .map(|range| range.end)
        .unwrap_or(0);

    let mut inserted = false;
    let mut cursor = 0usize;

    for range in ranges {
        if !inserted && cursor <= insert_after && insert_after <= range.start {
            output.extend_from_slice(&xmp_box);
            inserted = true;
        }

        if range.kind == *b"uuid" && is_bmff_xmp_box(&input[range.start..range.end]) {
            cursor = range.end;
            continue;
        }

        output.extend_from_slice(&input[range.start..range.end]);
        cursor = range.end;
    }

    if !inserted {
        if insert_after <= input.len() {
            let mut rebuilt = Vec::with_capacity(output.len() + xmp_box.len());
            rebuilt.extend_from_slice(&output[..insert_after.min(output.len())]);
            rebuilt.extend_from_slice(&xmp_box);
            rebuilt.extend_from_slice(&output[insert_after.min(output.len())..]);
            output = rebuilt;
        } else {
            output.extend_from_slice(&xmp_box);
        }
    }

    fs::write(path, output).with_context(|| format!("unable to rewrite {}", path.display()))?;
    Ok(())
}

fn write_ffmpeg_metadata(path: &Path, payload: &str) -> Result<()> {
    let compact_payload = serde_json::from_str::<serde_json::Value>(payload)
        .ok()
        .and_then(|value| serde_json::to_string(&value).ok())
        .unwrap_or_else(|| payload.to_string());

    let parent = path
        .parent()
        .with_context(|| format!("missing parent folder for {}", path.display()))?;
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("media.bin");
    let temp_path = parent.join(format!(".{file_name}.doka-tmp"));

    info!(
        "🧰 Launch external tool [ffmpeg] for metadata injection [{}]",
        path.display()
    );
    let output = Command::new("ffmpeg")
        .arg("-v")
        .arg("error")
        .arg("-i")
        .arg(path)
        .arg("-map")
        .arg("0")
        .arg("-codec")
        .arg("copy")
        .arg("-metadata")
        .arg(format!("{DOKA_PROPERTY_NAME}={compact_payload}"))
        .arg("-y")
        .arg(&temp_path)
        .output()
        .with_context(|| format!("unable to launch ffmpeg for {}", path.display()))?;

    if !output.status.success() {
        let _ = fs::remove_file(&temp_path);
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!(
            "💣 External tool failed [ffmpeg] for metadata injection [{}] with status {}",
            path.display(),
            output.status
        );
        anyhow::bail!(
            "ffmpeg metadata update failed for {}: {}",
            path.display(),
            stderr.trim()
        );
    }

    info!(
        "✅ External tool succeeded [ffmpeg] for metadata injection [{}]",
        path.display()
    );

    fs::rename(&temp_path, path)
        .with_context(|| format!("unable to replace {}", path.display()))?;
    Ok(())
}

fn ensure_webp_vp8x_chunk(webp: &mut WebP) -> Result<()> {
    if let Some(index) = webp
        .chunks()
        .iter()
        .position(|chunk| chunk.id() == CHUNK_VP8X)
    {
        let chunk = &mut webp.chunks_mut()[index];
        let RiffContent::Data(data) = chunk.content_mut() else {
            anyhow::bail!("invalid VP8X chunk");
        };
        let mut bytes = data.to_vec();
        if bytes.len() < 10 {
            anyhow::bail!("invalid VP8X payload");
        }
        bytes[0] |= WEBP_XMP_FLAG;
        *data = bytes.into();
        return Ok(());
    }

    let (width, height) = webp
        .dimensions()
        .with_context(|| "unable to determine webp dimensions")?;
    let mut flags = 0u8;
    if webp.has_chunk(img_parts::webp::CHUNK_ICCP) {
        flags |= 0b0010_0000;
    }
    if webp.has_chunk(img_parts::webp::CHUNK_EXIF) {
        flags |= 0b0000_1000;
    }
    flags |= WEBP_XMP_FLAG;

    let mut contents = Vec::with_capacity(10);
    contents.extend_from_slice(&[flags, 0, 0, 0]);
    contents.extend_from_slice(&(width - 1).to_le_bytes()[..3]);
    contents.extend_from_slice(&(height - 1).to_le_bytes()[..3]);

    let chunk = RiffChunk::new(CHUNK_VP8X, RiffContent::Data(contents.into()));
    let insert_at = webp
        .chunks()
        .iter()
        .position(|chunk| chunk.id() == CHUNK_VP8 || chunk.id() == CHUNK_VP8L)
        .unwrap_or(0);
    webp.chunks_mut().insert(insert_at, chunk);
    Ok(())
}

fn write_openxml_custom_property(path: &Path, payload: &str) -> Result<()> {
    let input = fs::read(path).with_context(|| format!("unable to read {}", path.display()))?;
    let reader = Cursor::new(input);
    let mut archive = ZipArchive::new(reader)
        .with_context(|| format!("unable to open office file {}", path.display()))?;

    let mut output = Cursor::new(Vec::new());
    let mut writer = ZipWriter::new(&mut output);

    let mut custom_xml = None;
    let mut root_relationships = None;
    let mut content_types = None;

    for index in 0..archive.len() {
        let mut file = archive.by_index(index)?;
        let name = file.name().to_string();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;

        match name.as_str() {
            "docProps/custom.xml" => {
                custom_xml = Some(String::from_utf8(bytes).context("invalid custom.xml")?);
                continue;
            }
            "_rels/.rels" => {
                root_relationships = Some(String::from_utf8(bytes).context("invalid .rels")?);
                continue;
            }
            "[Content_Types].xml" => {
                content_types =
                    Some(String::from_utf8(bytes).context("invalid [Content_Types].xml")?);
                continue;
            }
            _ => {}
        }

        let options = SimpleFileOptions::default()
            .compression_method(file.compression())
            .unix_permissions(file.unix_mode().unwrap_or(0o644));
        writer.start_file(name, options)?;
        writer.write_all(&bytes)?;
    }

    let custom_xml = upsert_docx_custom_property(custom_xml.as_deref(), payload)?;
    let root_relationships = upsert_docx_relationship(root_relationships.as_deref())?;
    let content_types = upsert_docx_content_type(content_types.as_deref())?;

    let xml_options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    writer.start_file("[Content_Types].xml", xml_options)?;
    writer.write_all(content_types.as_bytes())?;

    writer.start_file("_rels/.rels", xml_options)?;
    writer.write_all(root_relationships.as_bytes())?;

    writer.start_file("docProps/custom.xml", xml_options)?;
    writer.write_all(custom_xml.as_bytes())?;

    writer.finish()?;
    fs::write(path, output.into_inner())
        .with_context(|| format!("unable to rewrite {}", path.display()))?;
    Ok(())
}

fn write_epub_custom_metadata(path: &Path, payload: &str) -> Result<()> {
    let input = fs::read(path).with_context(|| format!("unable to read {}", path.display()))?;
    let reader = Cursor::new(input);
    let mut archive = ZipArchive::new(reader)
        .with_context(|| format!("unable to open epub {}", path.display()))?;

    let mut output = Cursor::new(Vec::new());
    let mut writer = ZipWriter::new(&mut output);

    let mut container_xml = None;
    let mut entries = Vec::new();

    for index in 0..archive.len() {
        let mut file = archive.by_index(index)?;
        let name = file.name().to_string();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let compression = file.compression();
        let permissions = file.unix_mode().unwrap_or(0o644);
        entries.push((name, bytes, compression, permissions));
    }

    for (name, bytes, _, _) in &entries {
        if name == "META-INF/container.xml" {
            container_xml =
                Some(String::from_utf8(bytes.clone()).context("invalid container.xml")?);
            break;
        }
    }

    let container_xml = container_xml.context("missing META-INF/container.xml in epub")?;
    let package_path = find_epub_package_path(&container_xml)?;
    let package_xml = entries
        .iter()
        .find(|(name, _, _, _)| name == &package_path)
        .map(|(_, bytes, _, _)| {
            String::from_utf8(bytes.clone()).context("invalid package document")
        })
        .transpose()?
        .context("missing OPF package document in epub")?;
    let package_xml = upsert_epub_package_metadata(&package_xml, payload)?;

    for (name, bytes, compression, permissions) in entries {
        if name == "META-INF/container.xml" || name == package_path {
            continue;
        }
        let options = SimpleFileOptions::default()
            .compression_method(compression)
            .unix_permissions(permissions);
        writer.start_file(name, options)?;
        writer.write_all(&bytes)?;
    }

    let xml_options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    writer.start_file("META-INF/container.xml", xml_options)?;
    writer.write_all(container_xml.as_bytes())?;
    writer.start_file(package_path, xml_options)?;
    writer.write_all(package_xml.as_bytes())?;

    writer.finish()?;
    fs::write(path, output.into_inner())
        .with_context(|| format!("unable to rewrite {}", path.display()))?;
    Ok(())
}

fn write_opendocument_custom_property(path: &Path, payload: &str) -> Result<()> {
    let input = fs::read(path).with_context(|| format!("unable to read {}", path.display()))?;
    let reader = Cursor::new(input);
    let mut archive = ZipArchive::new(reader)
        .with_context(|| format!("unable to open open document file {}", path.display()))?;

    let mut output = Cursor::new(Vec::new());
    let mut writer = ZipWriter::new(&mut output);
    let mut meta_xml = None;

    for index in 0..archive.len() {
        let mut file = archive.by_index(index)?;
        let name = file.name().to_string();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;

        if name == "meta.xml" {
            meta_xml = Some(String::from_utf8(bytes).context("invalid meta.xml")?);
            continue;
        }

        let options = SimpleFileOptions::default()
            .compression_method(file.compression())
            .unix_permissions(file.unix_mode().unwrap_or(0o644));
        writer.start_file(name, options)?;
        writer.write_all(&bytes)?;
    }

    let meta_xml = upsert_opendocument_meta(meta_xml.as_deref(), payload)?;
    let xml_options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    writer.start_file("meta.xml", xml_options)?;
    writer.write_all(meta_xml.as_bytes())?;

    writer.finish()?;
    fs::write(path, output.into_inner())
        .with_context(|| format!("unable to rewrite {}", path.display()))?;
    Ok(())
}

fn write_rtf_custom_metadata(path: &Path, payload: &str) -> Result<()> {
    let rtf =
        fs::read_to_string(path).with_context(|| format!("unable to read {}", path.display()))?;
    let comment_text = format!("{}:{}", DOKA_PROPERTY_NAME, payload);
    let escaped_comment = escape_rtf_text(&comment_text);
    let comment_group = format!(r#"{{\comment {}}}"#, escaped_comment);

    let existing_comment_regex = Regex::new(
        r#"(?s)\{\\info(?:(?!\{\\comment doka-custom:).)*\{\\comment doka-custom:.*?\}(?:(?!\}).)*\}"#,
    )?;
    let updated = if existing_comment_regex.is_match(&rtf) {
        existing_comment_regex
            .replace(&rtf, |caps: &regex::Captures<'_>| {
                let info = caps.get(0).map(|m| m.as_str()).unwrap_or("");
                let inner_comment_regex =
                    Regex::new(r#"(?s)\{\\comment doka-custom:.*?\}"#).expect("valid regex");
                inner_comment_regex
                    .replace(info, comment_group.as_str())
                    .into_owned()
            })
            .into_owned()
    } else if let Some(index) = rtf.find("{\\info") {
        if let Some(end) = find_matching_rtf_brace(&rtf, index) {
            let mut out = String::with_capacity(rtf.len() + comment_group.len() + 1);
            out.push_str(&rtf[..end]);
            out.push_str(&comment_group);
            out.push_str(&rtf[end..]);
            out
        } else {
            format!("{{\\info{}}}{}", comment_group, rtf)
        }
    } else if let Some(index) = rtf.find('{') {
        let insert_at = index + 1;
        let mut out = String::with_capacity(rtf.len() + comment_group.len() + 7);
        out.push_str(&rtf[..insert_at]);
        out.push_str("{\\info");
        out.push_str(&comment_group);
        out.push('}');
        out.push_str(&rtf[insert_at..]);
        out
    } else {
        anyhow::bail!("invalid rtf structure in {}", path.display());
    };

    fs::write(path, updated).with_context(|| format!("unable to rewrite {}", path.display()))?;
    Ok(())
}

fn upsert_docx_content_type(existing: Option<&str>) -> Result<String> {
    let xml = existing.context("missing [Content_Types].xml in office package")?;
    if xml.contains("PartName=\"/docProps/custom.xml\"") {
        return Ok(xml.to_string());
    }

    Ok(xml.replace(
        "</Types>",
        "  <Override PartName=\"/docProps/custom.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.custom-properties+xml\"/>\n</Types>",
    ))
}

fn upsert_docx_relationship(existing: Option<&str>) -> Result<String> {
    let xml = existing.context("missing _rels/.rels in office package")?;
    if xml.contains("Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/custom-properties\"") {
        return Ok(xml.to_string());
    }

    let relationship_id = if xml.contains("Id=\"rIdDokaCustom\"") {
        "rIdDokaCustom1"
    } else {
        "rIdDokaCustom"
    };

    Ok(xml.replace(
        "</Relationships>",
        &format!(
            "  <Relationship Id=\"{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/custom-properties\" Target=\"docProps/custom.xml\"/>\n</Relationships>",
            relationship_id
        ),
    ))
}

fn upsert_docx_custom_property(existing: Option<&str>, payload: &str) -> Result<String> {
    let escaped_payload = encode_safe(payload).to_string();
    let property = |pid: u32| {
        format!(
            "  <property fmtid=\"{{D5CDD505-2E9C-101B-9397-08002B2CF9AE}}\" pid=\"{}\" name=\"{}\"><vt:lpwstr>{}</vt:lpwstr></property>\n",
            pid, DOKA_PROPERTY_NAME, escaped_payload
        )
    };

    match existing {
        Some(xml) => {
            let property_regex =
                Regex::new(r#"(?s)\s*<property\b[^>]*\bname="doka-custom"[^>]*>.*?</property>"#)?;
            let cleaned = property_regex.replace_all(xml, "");
            let pid_regex = Regex::new(r#"pid="(\d+)""#)?;
            let next_pid = pid_regex
                .captures_iter(&cleaned)
                .filter_map(|captures| captures.get(1))
                .filter_map(|capture| capture.as_str().parse::<u32>().ok())
                .max()
                .unwrap_or(1)
                + 1;
            Ok(cleaned.replace("</Properties>", &(property(next_pid) + "</Properties>")))
        }
        None => Ok(format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Properties xmlns=\"http://schemas.openxmlformats.org/officeDocument/2006/custom-properties\" xmlns:vt=\"http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes\">\n{}</Properties>",
            property(2)
        )),
    }
}

fn upsert_opendocument_meta(existing: Option<&str>, payload: &str) -> Result<String> {
    let xml = existing.context("missing meta.xml in open document package")?;
    let user_defined_regex = Regex::new(
        r#"(?s)\s*<meta:user-defined\b[^>]*\bmeta:name="doka-custom"[^>]*>.*?</meta:user-defined>"#,
    )?;
    let cleaned = user_defined_regex.replace_all(xml, "").into_owned();
    let escaped_payload = encode_safe(payload);
    let property = format!(
        "\n<meta:user-defined meta:name=\"{name}\" meta:value-type=\"string\">{payload}</meta:user-defined>",
        name = DOKA_PROPERTY_NAME,
        payload = escaped_payload,
    );

    if cleaned.contains("</office:meta>") {
        Ok(cleaned.replacen("</office:meta>", &(property + "\n</office:meta>"), 1))
    } else {
        anyhow::bail!("missing </office:meta> in open document metadata")
    }
}

#[derive(Clone, Copy)]
struct BmffBoxRange {
    start: usize,
    end: usize,
    kind: [u8; 4],
}

fn parse_bmff_top_level_boxes(bytes: &[u8]) -> Result<Vec<BmffBoxRange>> {
    let mut ranges = Vec::new();
    let mut offset = 0usize;

    while offset + 8 <= bytes.len() {
        let size = u32::from_be_bytes(bytes[offset..offset + 4].try_into().expect("slice"));
        let kind: [u8; 4] = bytes[offset + 4..offset + 8].try_into().expect("slice");
        let (box_len, header_len) = match size {
            0 => (bytes.len() - offset, 8usize),
            1 => {
                if offset + 16 > bytes.len() {
                    anyhow::bail!("invalid bmff large-size box");
                }
                let large =
                    u64::from_be_bytes(bytes[offset + 8..offset + 16].try_into().expect("slice"));
                let len = usize::try_from(large).context("bmff box too large")?;
                (len, 16usize)
            }
            value => (
                usize::try_from(value).context("bmff box too large")?,
                8usize,
            ),
        };

        if box_len < header_len || offset + box_len > bytes.len() {
            anyhow::bail!("invalid bmff box size");
        }

        ranges.push(BmffBoxRange {
            start: offset,
            end: offset + box_len,
            kind,
        });
        offset += box_len;
    }

    if offset != bytes.len() {
        anyhow::bail!("invalid trailing bmff data");
    }

    Ok(ranges)
}

fn is_bmff_xmp_box(bytes: &[u8]) -> bool {
    let header_len = if bytes.len() >= 16 && &bytes[4..8] == b"uuid" {
        if &bytes[..4] == [0, 0, 0, 1] { 16 } else { 8 }
    } else {
        return false;
    };

    bytes.len() >= header_len + 16 && bytes[header_len..header_len + 16] == BMFF_XMP_UUID
}

fn build_bmff_xmp_box(payload: &str) -> Result<Vec<u8>> {
    let xmp = build_xmp_packet(payload).into_bytes();
    let size = 8usize
        .checked_add(16)
        .and_then(|value| value.checked_add(xmp.len()))
        .context("bmff xmp box too large")?;
    let size_u32 = u32::try_from(size).context("bmff xmp box exceeds 32-bit size")?;

    let mut bytes = Vec::with_capacity(size);
    bytes.extend_from_slice(&size_u32.to_be_bytes());
    bytes.extend_from_slice(b"uuid");
    bytes.extend_from_slice(&BMFF_XMP_UUID);
    bytes.extend_from_slice(&xmp);
    Ok(bytes)
}

fn find_epub_package_path(container_xml: &str) -> Result<String> {
    let rootfile_regex = Regex::new(r#"full-path="([^"]+)""#)?;
    let path = rootfile_regex
        .captures(container_xml)
        .and_then(|captures| captures.get(1))
        .map(|capture| capture.as_str().to_string())
        .context("missing rootfile full-path in container.xml")?;
    Ok(path)
}

fn upsert_epub_package_metadata(package_xml: &str, payload: &str) -> Result<String> {
    let property_regex =
        Regex::new(r#"(?s)\s*<meta\b[^>]*\bname="doka-custom"[^>]*\bcontent="[^"]*"[^>]*/?>"#)?;
    let cleaned = property_regex.replace_all(package_xml, "").into_owned();
    let escaped_payload = encode_safe(payload);
    let property = format!(
        r#"<meta name="{name}" content="{payload}" />"#,
        name = DOKA_PROPERTY_NAME,
        payload = escaped_payload,
    );

    if cleaned.contains("</metadata>") {
        Ok(cleaned.replacen("</metadata>", &format!("  {property}\n</metadata>"), 1))
    } else {
        anyhow::bail!("missing </metadata> in epub package document")
    }
}

fn escape_rtf_text(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 16);
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str(r"\\"),
            '{' => escaped.push_str(r"\{"),
            '}' => escaped.push_str(r"\}"),
            '\n' => escaped.push_str(r"\par "),
            '\r' => {}
            ch if ch.is_ascii() => escaped.push(ch),
            ch => escaped.push_str(&format!(r"\u{}?", ch as i32)),
        }
    }
    escaped
}

fn find_matching_rtf_brace(text: &str, start: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut escaped = false;

    for (index, ch) in text.char_indices().skip_while(|(index, _)| *index < start) {
        if escaped {
            escaped = false;
            continue;
        }

        match ch {
            '\\' => escaped = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }

    None
}
