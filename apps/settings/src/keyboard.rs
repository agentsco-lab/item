//! The keyboard: item's own or the port's stevia, its languages, whether
//! it rises for a text field by itself, the tick under a finger; and the
//! voice (the microphone key): the model and the language, whether the
//! OpenRouter key is in the keyring. Each setting is one small file in
//! ~/.config/item (the compositor's prefs.rs reads them again within a
//! second; `keyboard` from item's next start), the voice's in its
//! `voice` file (voice.rs).

use adw::prelude::*;

use crate::ui::{group, read, row, run, write_conf};

const VOICE_MODELS: &[(&str, &str)] = &[
    ("elevenlabs/scribe-v2", "ElevenLabs Scribe v2 · most accurate"),
    ("mistralai/voxtral-mini-transcribe-3-0", "Mistral Voxtral Mini · fastest"),
    ("openai/gpt-4o-mini-transcribe", "OpenAI GPT-4o mini · cheapest"),
];
const VOICE_LANGS: &[(&str, &str)] = &[("en", "English"), ("ru", "Russian")];

pub fn home_conf(name: &str) -> String {
    read(&format!("{}/.config/item/{name}", std::env::var("HOME").unwrap_or_default()))
}

/// The voice file's keys, as they are.
fn voice_conf() -> Vec<(String, String)> {
    home_conf("voice").lines().filter_map(|l| l.split_once('=').map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))).collect()
}

fn voice_get(key: &str) -> String {
    voice_conf().into_iter().find(|(k, _)| k == key).map(|(_, v)| v).unwrap_or_default()
}

/// One key of the voice file set, the rest kept.
fn voice_set(key: &str, value: &str) {
    let mut lines = voice_conf();
    match lines.iter_mut().find(|(k, _)| k == key) {
        Some(l) => l.1 = value.to_owned(),
        None => lines.push((key.to_owned(), value.to_owned())),
    }
    let text: String = lines.iter().map(|(k, v)| format!("{k} = {v}\n")).collect();
    write_conf("voice", &text);
}

/// Rows write only what a finger changed: a notification before the page
/// is on screen (the widgets settling as they are built - one wrote "en"
/// on the first run, 2026-10-21) or with the value already there is let
/// go.
pub fn switch(title: &str, subtitle: &str, on: bool, set: impl Fn(bool) + 'static) -> adw::SwitchRow {
    let r = adw::SwitchRow::builder().title(title).subtitle(subtitle).active(on).use_markup(false).build();
    let last = std::cell::Cell::new(on);
    r.connect_active_notify(move |r| {
        let now = r.is_active();
        if r.is_mapped() && now != last.get() {
            last.set(now);
            set(now);
        }
    });
    r
}

fn combo(title: &str, subtitle: &str, names: &[&str], selected: usize, set: impl Fn(usize) + 'static) -> adw::ComboRow {
    let r = adw::ComboRow::builder().title(title).subtitle(subtitle).use_markup(false).build();
    r.set_model(Some(&gtk::StringList::new(names)));
    r.set_selected(selected as u32);
    let last = std::cell::Cell::new(selected);
    r.connect_selected_notify(move |r| {
        let now = r.selected() as usize;
        if r.is_mapped() && now != last.get() {
            last.set(now);
            set(now);
        }
    });
    r
}

pub fn page() -> gtk::Widget {
    let page = adw::PreferencesPage::new();

    let g = group("Keyboard");
    let stevia = home_conf("keyboard") == "stevia";
    g.add(&combo("Keyboard", "item's own from the next start; stevia is the port's", &["item's own", "stevia"], stevia as usize, |i| {
        write_conf("keyboard", if i == 1 { "stevia\n" } else { "item\n" });
    }));
    let langs = home_conf("keyboard-langs");
    let ru = langs.is_empty() || langs.split_whitespace().any(|w| w == "ru");
    g.add(&switch("Russian", "English stays; the key between them goes when it is alone", ru, |on| {
        write_conf("keyboard-langs", if on { "en ru\n" } else { "en\n" });
    }));
    g.add(&switch("Rises for a text field", "A tap into a field brings it up; it goes when the field is left", home_conf("keyboard-auto") != "off", |on| {
        write_conf("keyboard-auto", if on { "on\n" } else { "off\n" });
    }));
    g.add(&switch("Vibration", "A tick under the finger on each key", home_conf("haptic") != "off", |on| {
        write_conf("haptic", if on { "on\n" } else { "off\n" });
    }));
    page.add(&g);

    let g = group("Voice");
    let model = voice_get("model");
    let mut names: Vec<&str> = VOICE_MODELS.iter().map(|(_, n)| *n).collect();
    let mut ids: Vec<String> = VOICE_MODELS.iter().map(|(id, _)| (*id).to_owned()).collect();
    if !model.is_empty() && !ids.contains(&model) {
        names.push(&model);
        ids.push(model.clone());
    }
    let selected = ids.iter().position(|id| *id == model).unwrap_or(0);
    g.add(&combo("Model", "Through OpenRouter, what the microphone key hears", &names, selected, move |i| voice_set("model", &ids[i])));
    let lang = voice_get("language");
    let selected = VOICE_LANGS.iter().position(|(id, _)| *id == lang).unwrap_or(0);
    let lang_names: Vec<&str> = VOICE_LANGS.iter().map(|(_, n)| *n).collect();
    g.add(&combo("Language", "What is spoken", &lang_names, selected, |i| voice_set("language", VOICE_LANGS[i].0)));
    let keys = run(&["item-secret", "list"]);
    let has_key = keys.lines().any(|l| l.trim() == "openrouter");
    g.add(&row("OpenRouter key", if has_key { "in the keyring" } else { "none · put it in with item/grid" }));
    page.add(&g);

    page.upcast()
}
