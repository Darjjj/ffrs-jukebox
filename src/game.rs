//! Everything that talks to the game. Runs on the game thread from a hook at
//! the entry of `UObject::ProcessEvent`, throttled to [`TICK_MS`], because
//! UFunctions may only be called from there.

use std::collections::HashMap;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use sdk::CriWareRuntime::{FAtomCueInfo, UManaPlayer, USoundAtomCueSheet};
use sdk::Engine::USubsystemBlueprintLibrary;
use sdk::UMG::{UEditableText, UEditableTextBox, UMultiLineEditableText, UMultiLineEditableTextBox, UWidget};
use sdk::mucha::{
    EeSoundCategory, EeSoundVolumeType, FPLAY_SOUND_PARAMETER, UCPP_BFL_Battle, UCPP_BFL_TalkEvent, UCPP_SoundManager,
    UCPP_UI_BattleSubsystem,
};
use sdk::rt::{self, FAllocatedString, TSubclassOf, UClassType};
use windows::Win32::System::SystemInformation::GetTickCount64;
use windows::Win32::System::Threading::GetCurrentThreadId;

use crate::config::Scene;
use crate::player::Player;
use crate::state::{self, Track};

const TICK_MS: u64 = 50;

pub static GAME_THREAD: AtomicU32 = AtomicU32::new(0);
static LAST_TICK: AtomicU64 = AtomicU64::new(0);
static BUSY: AtomicBool = AtomicBool::new(false);

/// Called at every ProcessEvent entry. Cheap unless a tick is due.
pub fn on_process_event() {
    if unsafe { GetCurrentThreadId() } != GAME_THREAD.load(Ordering::Relaxed) {
        return;
    }
    let now = unsafe { GetTickCount64() };
    if now.wrapping_sub(LAST_TICK.load(Ordering::Relaxed)) < TICK_MS {
        return;
    }
    // The tick calls UFunctions, which re-enter this hook.
    if BUSY.swap(true, Ordering::Acquire) {
        return;
    }
    LAST_TICK.store(now, Ordering::Relaxed);
    if std::panic::catch_unwind(tick).is_err() {
        log::error!("tick: panicked, see the panic line above");
    }
    BUSY.store(false, Ordering::Release);
}

thread_local! {
    static PLAYER: std::cell::RefCell<Option<Player>> = const { std::cell::RefCell::new(None) };
}

fn tick() {
    PLAYER.with_borrow_mut(|player| {
        let player = player.get_or_insert_with(Player::new);
        player.tick(&mut Game::get());
    });
}

/// The game's sound manager, found once and kept: it is a game-instance
/// subsystem, so it lives as long as the process.
static SOUND_MANAGER: std::sync::atomic::AtomicPtr<UCPP_SoundManager> =
    std::sync::atomic::AtomicPtr::new(null_mut());
static NEXT_SEARCH: AtomicU64 = AtomicU64::new(0);
/// When a story event was last seen playing, and how long a gap between two
/// of its parts still counts as the event: the game reports no event for a
/// moment between them.
static EVENT_SEEN: AtomicU64 = AtomicU64::new(0);
const EVENT_GAP_MS: u64 = 500;
/// The same for everyday talk, whose parts follow each other with no gap,
/// so the mini player comes back as soon as it ends.
const TALK_GAP_MS: u64 = 100;

fn sound_manager() -> *mut UCPP_SoundManager {
    let found = SOUND_MANAGER.load(Ordering::Relaxed);
    if !found.is_null() {
        return found;
    }
    // A full GObjects walk; only retried every two seconds until it succeeds.
    let now = unsafe { GetTickCount64() };
    if now < NEXT_SEARCH.load(Ordering::Relaxed) {
        return null_mut();
    }
    NEXT_SEARCH.store(now + 2000, Ordering::Relaxed);
    let class = UCPP_SoundManager::static_class();
    for obj in rt::GObjects.iter() {
        let o = unsafe { &*obj };
        if o.Class == class && !o.IsDefaultObject() {
            log::info!("game: sound manager at {obj:p}");
            SOUND_MANAGER.store(obj.cast(), Ordering::Relaxed);
            return obj.cast();
        }
    }
    null_mut()
}

/// Objects the jukebox watches for, found by walking GObjects a slice at a
/// time: a whole walk at once stalled the game thread for milliseconds.
/// Each is kept as (GObjects slot, pointer), so one that was destroyed is
/// noticed by its slot no longer holding it.
#[derive(Default)]
struct Watch {
    /// Movie players, for movies with their own soundtrack.
    players: Vec<(i32, usize)>,
    /// The game's text boxes, so hotkeys stay quiet while you type in them.
    text_boxes: Vec<(i32, usize)>,
    // The walk in progress; swapped in when it reaches the end.
    next_players: Vec<(i32, usize)>,
    next_text_boxes: Vec<(i32, usize)>,
    pos: i32,
    /// What each class seen so far is: a few thousand classes, against
    /// hundreds of thousands of objects.
    kinds: HashMap<usize, Kind>,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Player,
    TextBox,
    Other,
}

/// Objects walked per tick: a full pass takes well under a second.
const WATCH_SLICE: i32 = 25_000;

static WATCH: std::sync::Mutex<Option<Watch>> = std::sync::Mutex::new(None);

fn kind_of(class: *mut sdk::config::UClass) -> Kind {
    let text_classes = [
        UEditableText::static_class(),
        UEditableTextBox::static_class(),
        UMultiLineEditableText::static_class(),
        UMultiLineEditableTextBox::static_class(),
    ];
    let o = unsafe { &*class.cast::<sdk::config::UStruct>() };
    if o.IsSubclassOf(UManaPlayer::static_class()) {
        Kind::Player
    } else if text_classes.iter().any(|&c| !c.is_null() && o.IsSubclassOf(c)) {
        Kind::TextBox
    } else {
        Kind::Other
    }
}

/// Walks the next slice of GObjects.
fn watch_step(w: &mut Watch) {
    let num = rt::GObjects.Num();
    let end = (w.pos + WATCH_SLICE).min(num);
    for index in w.pos..end {
        let obj = rt::GObjects.GetByIndex(index);
        let Some(o) = (unsafe { obj.as_ref() }) else { continue };
        if o.Class.is_null() || o.IsDefaultObject() {
            continue;
        }
        let kind = *w.kinds.entry(o.Class as usize).or_insert_with(|| kind_of(o.Class));
        match kind {
            Kind::Player => w.next_players.push((index, obj as usize)),
            Kind::TextBox => w.next_text_boxes.push((index, obj as usize)),
            Kind::Other => {}
        }
    }
    w.pos = end;
    if end >= num {
        w.players = std::mem::take(&mut w.next_players);
        w.text_boxes = std::mem::take(&mut w.next_text_boxes);
        w.pos = 0;
    }
}

fn alive(&(index, ptr): &(i32, usize)) -> bool {
    rt::GObjects.GetByIndex(index) as usize == ptr
}

/// Set each tick: one of the game's text boxes has the keyboard.
pub static GAME_TYPING: AtomicBool = AtomicBool::new(false);

/// Advances the watch, and answers whether a movie with its own soundtrack
/// is playing. Also updates [`GAME_TYPING`].
fn watch() -> bool {
    let mut guard = WATCH.lock().unwrap_or_else(|e| e.into_inner());
    let w = guard.get_or_insert_with(Watch::default);
    watch_step(w);
    let typing = w.text_boxes.iter().filter(|e| alive(e)).any(|&(_, ptr)| {
        unsafe { &mut *(ptr as *mut UWidget) }.HasKeyboardFocus()
    });
    GAME_TYPING.store(typing, Ordering::Relaxed);
    w.players.iter().filter(|e| alive(e)).any(|&(_, ptr)| {
        let player = unsafe { &mut *(ptr as *mut UManaPlayer) };
        player.IsPlaying() && unsafe { player.ManaMovie.as_ref() }.is_some_and(|m| m.AudioTracks.Num() > 0)
    })
}

thread_local! {
    /// The current event's id and whether it is everyday talk.
    static LAST_EVENT: std::cell::RefCell<(String, bool)> = const { std::cell::RefCell::new((String::new(), false)) };
}

/// Event categories that are everyday talk rather than story: townsfolk
/// ("AtcRestartTalk"), shops, inns and upgrades ("Shop"), boarding the airship
/// ("Gimmick"), notices on a bulletin board ("SignboardGimmick") and the
/// summon trainers ("Summon"). Story scenes are filed by chapter
/// ("Event.Category.C01"); anything not listed here still counts as one.
const TALK_CATEGORIES: [&str; 5] = [
    "Event.Category.AtcRestartTalk",
    "Event.Category.Shop",
    "Event.Category.Gimmick",
    "Event.Category.SignboardGimmick",
    "Event.Category.Summon",
];

/// When everyday talk was last seen. A bulletin board's list, then a notice,
/// then the list again follow each other within a tick, so only a short
/// [`TALK_GAP_MS`] bridges them.
static TALK_SEEN: AtomicU64 = AtomicU64::new(0);

/// Whether the player is talking to someone or reading a sign, for hiding
/// the mini player during it.
pub fn talking() -> bool {
    unsafe { GetTickCount64() }.wrapping_sub(TALK_SEEN.load(Ordering::Relaxed)) < TALK_GAP_MS
}

/// Whether the playing event is everyday talk (an NPC, a shop) rather than a
/// story scene. Looked up once per event, and logged then with its id, type,
/// category and description.
fn event_is_talk() -> bool {
    let id = UCPP_BFL_TalkEvent::GetCurrentEventId().ToString();
    if let Some(talk) = LAST_EVENT.with_borrow(|(last, talk)| (*last == id).then_some(*talk)) {
        return talk;
    }
    let data = UCPP_BFL_TalkEvent::GetCurrentEventData();
    let sequence = UCPP_BFL_TalkEvent::GetCurrentSequenceData();
    let category = data.Category.TagName.ToString();
    let talk = TALK_CATEGORIES.contains(&category.as_str());
    log::info!(
        "game: event {id} type={:?} category={category} sequence={} talk={talk} desc={}",
        data.EventType,
        sequence.Name.ToString(),
        data.Description,
    );
    LAST_EVENT.set((id, talk));
    talk
}

/// The battle UI's own flag. The encounter's battle group id is not usable:
/// it keeps the last encounter's id after the battle ends.
fn during_battle() -> bool {
    let world = sdk::config::UWorld::GetWorld();
    if world.is_null() {
        return false;
    }
    let ui = USubsystemBlueprintLibrary::GetWorldSubsystem(
        world.cast(),
        TSubclassOf::new(UCPP_UI_BattleSubsystem::static_class()),
    )
    .cast::<UCPP_UI_BattleSubsystem>();
    unsafe { ui.as_ref() }.is_some_and(|ui| ui.bDuringBattle)
}

/// A handle on the game for one tick. Every method is a no-op until the sound
/// manager exists.
pub struct Game {
    sm: *mut UCPP_SoundManager,
}

impl Game {
    fn get() -> Self {
        Self { sm: sound_manager() }
    }

    pub fn ready(&self) -> bool {
        !self.sm.is_null()
    }

    fn sm(&mut self) -> Option<&mut UCPP_SoundManager> {
        unsafe { self.sm.as_mut() }
    }

    /// What the game is doing. Battle and event checks only run once the
    /// sound manager exists, i.e. past engine start-up.
    pub fn scene(&mut self) -> Scene {
        // A pre-rendered movie with its own soundtrack (the opening, for
        // one) can't be muted like the game's music, so the jukebox steps
        // aside for it. Silent movies play the game's music tracks instead,
        // which the jukebox replaces as usual.
        let movie = watch();
        if !self.ready() {
            return if movie { Scene::Movie } else { Scene::Exploring };
        }
        let battle = during_battle();
        // A movie inside a battle (a Vision attack) plays over the game's
        // battle music, so it stays part of the battle and the song carries on.
        if movie && !battle {
            crate::log_change("signals", "game: movie with its own audio".into());
            return Scene::Movie;
        }
        crate::log_change("movie", format!("game: movie in battle={movie}"));
        // Talking to townsfolk or a shop is part of exploring, so the music
        // carries on through it; only story scenes count as events.
        let playing = UCPP_BFL_TalkEvent::IsPlayingEvent();
        let talk = playing && event_is_talk();
        if talk {
            TALK_SEEN.store(unsafe { GetTickCount64() }, Ordering::Relaxed);
        }
        let event = playing && !talk;
        crate::log_change("signals", format!("game: event={event} battle={battle}"));
        let now = unsafe { GetTickCount64() };
        // Only story events outside battle; a battle's own dialogue ends with it.
        if event && !battle {
            EVENT_SEEN.store(now, Ordering::Relaxed);
        }
        let event = event || now.wrapping_sub(EVENT_SEEN.load(Ordering::Relaxed)) < EVENT_GAP_MS;
        // A battle's intro and in-battle dialogue also count as an event;
        // they belong to the battle, so its music is not handed back for them.
        if battle {
            Scene::Battle
        } else if event {
            Scene::Event
        } else {
            Scene::Exploring
        }
    }

    /// Whether the current battle is one the game marks as a boss battle.
    /// Only meaningful while `scene()` says Battle.
    pub fn boss_battle(&mut self) -> bool {
        if !self.ready() {
            return false;
        }
        let boss = UCPP_BFL_Battle::IsBossBattle();
        crate::log_change("boss", format!("game: boss battle={boss}"));
        boss
    }

    fn bgm_sheet_name() -> String {
        UCPP_SoundManager::GetCueSheetNameBGM().ToString()
    }

    fn bgm_sheet(&mut self) -> *mut USoundAtomCueSheet {
        let name = Self::bgm_sheet_name();
        let Some(sm) = self.sm() else { return null_mut() };
        for map in [&sm.m_pSoundAtomCueSheetMap, &sm.m_pParmanentSoundAtomCueSheetMap] {
            if let Some(pair) = map.iter().find(|p| p.Key.ToString() == name) {
                return pair.Value;
            }
        }
        null_mut()
    }

    /// The game's songs, or `None` while the BGM cue sheet is not loaded.
    pub fn list_songs(&mut self) -> Option<Vec<Track>> {
        let sheet = unsafe { self.bgm_sheet().as_mut()? };
        let count = sheet.GetNumCues();
        log::info!("game: BGM cue sheet '{}' has {count} cues", Self::bgm_sheet_name());
        let mut tracks = Vec::new();
        for i in 0..count {
            let mut info: FAtomCueInfo = unsafe { std::mem::zeroed() };
            if !sheet.BP_GetCueInfoFromIndex(i, &mut info) {
                continue;
            }
            // The strings in `info` belong to the engine allocator; they are
            // read once at start-up and left alone.
            let cue = info.Name.ToString();
            // A silent placeholder cue.
            if cue.is_empty() || cue == "BGM_PP_DUMMY" {
                continue;
            }
            let secs = info.Duration.Ticks as f64 / 10_000_000.0;
            // Categories and AISAC controls are logged to find out why game
            // songs play quieter through the jukebox than the game plays them:
            // a control the game sets for its own music would show up here.
            let names = |a: &rt::TArray<rt::FString>| {
                a.as_slice().iter().map(|s| s.ToString()).collect::<Vec<_>>().join(",")
            };
            log::debug!(
                "game: cue {i}: {cue} ({secs:.1}s, loop={}, categories=[{}], aisac=[{}])",
                info.bLooping(),
                names(&info.CategoryNames),
                names(&info.AisacControlNames)
            );
            tracks.push(Track::game(cue, info.bLooping()));
        }
        Some(tracks)
    }

    /// How long one play of `cue` lasts. A looping theme plays on until
    /// stopped (CRI reports it as 10000 s), so it gets `loop_secs`, the
    /// "Per theme" setting. Its first wave is only one pass through the
    /// loop, and stopping there cut themes off after a minute or two.
    pub fn cue_duration(&mut self, cue: &str, looping: bool, loop_secs: f32) -> Option<f32> {
        if looping {
            return Some(loop_secs);
        }
        let mut sheet = FAllocatedString::from(Self::bgm_sheet_name().as_str());
        let mut name = FAllocatedString::from(cue);
        let d = self.sm()?.GetDuration(&sheet.Get(), &name.Get());
        (d > 0.0).then_some(d)
    }

    pub fn fade_out(&mut self, id: i32, secs: f32) {
        if let Some(sm) = self.sm() {
            sm.FadeOutSound(id, secs);
        }
    }

    pub fn play_cue(&mut self, cue: &str, start: f32) -> Option<i32> {
        let mut sheet = FAllocatedString::from(Self::bgm_sheet_name().as_str());
        let mut cue = FAllocatedString::from(cue);
        let mut param: FPLAY_SOUND_PARAMETER = unsafe { std::mem::zeroed() };
        param.startTime = start;
        param.isPersistAcrossLevelTransition = true;
        let id = self.sm()?.PlaySound2D(&sheet.Get(), &cue.Get(), &param);
        (id >= 0).then_some(id)
    }

    pub fn exists(&mut self, id: i32) -> bool {
        self.sm().is_some_and(|sm| sm.IsExistPlaySound(id))
    }
    pub fn time(&mut self, id: i32) -> f32 {
        self.sm().map_or(0.0, |sm| sm.GetTime(id))
    }
    pub fn set_volume(&mut self, id: i32, v: f32) {
        if let Some(sm) = self.sm() {
            sm.SetVolume(id, v);
        }
    }
    pub fn volume(&mut self, id: i32) -> f32 {
        self.sm().map_or(1.0, |sm| sm.GetVolume(id))
    }
    pub fn pause(&mut self, id: i32) {
        if let Some(sm) = self.sm() {
            sm.PauseSound(id);
        }
    }
    pub fn resume(&mut self, id: i32) {
        if let Some(sm) = self.sm() {
            sm.ResumeSound(id);
        }
    }
    pub fn stop(&mut self, id: i32) {
        if let Some(sm) = self.sm() {
            sm.StopSound(id);
        }
    }
    /// How loud the game plays its music overall, 0.0 - 1.0: its Music
    /// volume and its Master volume together. Game songs get both from the
    /// game; files are multiplied by this so they follow the same settings.
    /// Your files skipping a lowered Master is what made them sound so much
    /// louder than game songs. 1.0 before the game is ready.
    pub fn music_volume(&mut self) -> f32 {
        let Some(sm) = self.sm() else { return 1.0 };
        let bgm = unit(sm.GetSoundTotalVolumeForCategory(EeSoundCategory::BGM));
        let master = unit(sm.GetSoundTotalVolumeForCategory(EeSoundCategory::Master));
        let bgm_option = unit(sm.GetSoundVolumeForCategory(EeSoundCategory::BGM, EeSoundVolumeType::Option));
        let master_option = unit(sm.GetSoundVolumeForCategory(EeSoundCategory::Master, EeSoundVolumeType::Option));
        // Whether the game's total for Music already has Master in it: it
        // does if it reads Music x Master (only telling while Master is down).
        let includes_master = master < 0.99 && (bgm - bgm_option * master_option).abs() < 0.01;
        let v = if includes_master { bgm } else { bgm * master };
        crate::log_change(
            "music_volume",
            format!(
                "game: Music total {bgm:.3} (option {bgm_option:.3}), Master total {master:.3} \
                 (option {master_option:.3}) -> files at {v:.3}"
            ),
        );
        v
    }

    pub fn current_bgm(&mut self) -> Option<i32> {
        let id = self.sm()?.GetCurrentPlayBGMId();
        (id >= 0).then_some(id)
    }

    /// The cue name of the game's current BGM. Looked up once per play id:
    /// the name comes back as a string the engine allocates.
    pub fn current_bgm_cue(&mut self) -> Option<String> {
        let id = self.current_bgm()?;
        let mut cached = BGM_CUE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((known, name)) = cached.as_ref() {
            if *known == id {
                return Some(name.clone());
            }
        }
        let sound = *self.sm()?.m_PlaySoundList.get(&id)?;
        let atom = unsafe { sound.as_mut()?.m_AtomComponent.as_mut()? };
        let name = atom.GetCueName().ToString();
        log::info!("game: game BGM id {id} is {name}");
        *cached = Some((id, name.clone()));
        Some(name)
    }
}

/// The game's victory fanfare, played on the results after a won battle.
pub const VICTORY_CUE: &str = "BGM_PP_BATTLEEND";

/// The last BGM play id looked up by [`Game::current_bgm_cue`], and its cue.
static BGM_CUE: std::sync::Mutex<Option<(i32, String)>> = std::sync::Mutex::new(None);

/// A game volume as 0.0 - 1.0, in case the game keeps it as a 0-10 or
/// 0-100 setting.
fn unit(raw: f32) -> f32 {
    let v = if raw > 10.0 { raw / 100.0 } else if raw > 1.0 { raw / 10.0 } else { raw };
    v.clamp(0.0, 1.0)
}

/// Mutes whatever BGM the game is playing while the jukebox covers it, and
/// puts back the volume each muted track had before.
#[derive(Default)]
pub struct BgmMute {
    muted: HashMap<i32, f32>,
}

impl BgmMute {
    pub fn hold(&mut self, game: &mut Game, own: Option<i32>) {
        let Some(id) = game.current_bgm() else { return };
        if Some(id) == own {
            return;
        }
        if !self.muted.contains_key(&id) {
            let v = game.volume(id);
            // A track caught during its fade-in reads near silent; restoring
            // that would leave the game's music quiet.
            self.muted.insert(id, if v < 0.3 { 1.0 } else { v });
            log::info!("game: muting game BGM id {id} (was {v:.2})");
        }
        game.set_volume(id, 0.0);
    }

    pub fn release(&mut self, game: &mut Game) {
        for (id, v) in self.muted.drain() {
            if game.exists(id) {
                game.set_volume(id, v);
                log::info!("game: restored game BGM id {id} to {v:.2}");
            }
        }
    }
}

pub fn refresh_game_tracks(game: &mut Game) {
    if state::lock().game_tracks_ready || !game.ready() {
        return;
    }
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let now = unsafe { GetTickCount64() };
    if now < NEXT.load(Ordering::Relaxed) {
        return;
    }
    NEXT.store(now + 3000, Ordering::Relaxed);
    if let Some(songs) = game.list_songs() {
        if songs.is_empty() {
            return;
        }
        let mut s = state::lock();
        s.tracks.extend(songs);
        s.apply_titles();
        s.game_tracks_ready = true;
    }
}
