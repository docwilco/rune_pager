use anyhow::{anyhow, Context, Result};
use cached::proc_macro::cached;
use lcu::{LCUClient, LCUWebSocket};
use rusqlite::NO_PARAMS;
use rusqlite::{params, Connection, Row};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::str;
use std::sync::mpsc::channel;
use std::thread;
use std::time::{self, Duration};

mod lcu;

static MARKER: &str = "(RP)";

/* this provides static CHAMPIONS phf::Map<u64, &str> */
include!(concat!(env!("OUT_DIR"), "/champions.rs"));

#[derive(Serialize, Deserialize, Debug, Default, Clone)]
#[serde(rename_all = "camelCase")]
struct RunePage {
    auto_modified_selections: Vec<serde_json::Value>,
    current: bool,
    id: u64,
    is_active: bool,
    is_deletable: bool,
    is_editable: bool,
    is_valid: bool,
    last_modified: u64,
    name: String,
    order: u32,
    primary_style_id: i64,
    selected_perk_ids: Vec<i64>,
    sub_style_id: i64,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct RerollPoints {
    current_points: u32,
    max_rolls: u32,
    number_of_rolls: u32,
    points_cost_to_roll: u32,
    points_to_reroll: u32,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct LCUSummoner<'a> {
    account_id: u64,
    display_name: &'a str,
    internal_name: &'a str,
    percent_complete_for_next_level: u32,
    profile_icon_id: u64,
    puuid: &'a str,
    reroll_points: RerollPoints,
    summoner_id: u64,
    summoner_level: u32,
    xp_since_last_level: u32,
    xp_until_next_level: u32,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct RiotSummoner<'a> {
    id: &'a str,
    account_id: &'a str,
    puuid: &'a str,
    name: &'a str,
    profile_icon_id: u64,
    revision_date: u64,
    summoner_level: u32,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct MatchHistoryResult<'a> {
    #[serde(borrow)]
    matches: Vec<MatchHistoryMatch<'a>>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct MatchHistoryMatch<'a> {
    game_id: u64,
    platform_id: &'a str,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct Participant {
    participant_id: u64,
    spell1_id: u64,
    spell2_id: u64,
    stats: ParticipantStats,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct ParticipantStats {
    perk0: i64,
    perk1: i64,
    perk2: i64,
    perk3: i64,
    perk4: i64,
    perk5: i64,
    stat_perk0: i64,
    stat_perk1: i64,
    stat_perk2: i64,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct Player<'a> {
    platform_id: &'a str,
    account_id: &'a str,
    summoner_name: &'a str,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct ParticipantIdentity<'a> {
    participant_id: u64,
    #[serde(borrow)]
    player: Player<'a>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct MatchDetails<'a> {
    participants: Vec<Participant>,
    #[serde(borrow)]
    participant_identities: Vec<ParticipantIdentity<'a>>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct SelectSession {
    local_player_cell_id: u64,
    my_team: Vec<SessionPlayer>,
    their_team: Vec<SessionPlayer>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
#[serde(rename_all = "camelCase")]
struct SessionPlayer {
    cell_id: u64,
    champion_id: u64,
    champion_pick_intent: u64,
    spell1_id: u64,
    spell2_id: u64,
    summoner_id: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum GamePhase {
    Lobby,
    Matchmaking,
    ReadyCheck,
    ChampSelect,
    GameStart,
    InProgress,
    WaitingForStats,
    PreEndOfGame,
    EndOfGame,
}

fn setup_sqlite() -> Result<Connection> {
    let conn = Connection::open("rune_pages.db").context("failed to open DB file")?;

    let create = conn.execute(
        "create table rune_pages (
                champ_id integer not null,
                game_mode text not null,
                spell1_id integer not null,
                spell2_id integer not null,
                page text not null,
                primary key (champ_id, game_mode) on conflict replace
            )",
        NO_PARAMS,
    );
    // If create worked, we're done
    if create.is_ok() {
        return Ok(conn);
    }
    // If create didn't work, we can have either v1 or v2. Try to add a v2 column.
    let alter = conn.execute(
        "alter table rune_pages add column spell1_id integer",
        NO_PARAMS,
    );
    // If add column failed, we're on v2, so we're done.
    if alter.is_err() {
        return Ok(conn);
    }
    // Add column succeeded, so convert v1 to v2
    // see https://www.sqlite.org/lang_altertable.html for specifics on order of operations.
    println!("upgrading database");
    conn.execute(
        "create table rune_pages_new (
            champ_id integer not null,
            game_mode text not null,
            spell1_id integer not null,
            spell2_id integer not null,
            page text not null,
            primary key (champ_id, game_mode) on conflict replace
        )",
        NO_PARAMS,
    )?;

    conn.execute(
        "insert into rune_pages_new (champ_id, game_mode, spell1_id, spell2_id, page)
            select champ_id
            , game_mode
            , json_extract(player, '$.spell1Id')
            , json_extract(player, '$.spell2Id')
            , page
            from rune_pages",
        NO_PARAMS,
    )?;
    conn.execute("drop table rune_pages", NO_PARAMS)?;
    conn.execute("alter table rune_pages_new rename to rune_pages", NO_PARAMS)?;
    println!("upgraded database");
    Ok(conn)
}

fn save_rune_page(
    conn: &Connection,
    champ_id: u64,
    spells: (u64, u64),
    game_mode: &str,
    rune_page: &RunePage,
) -> Result<()> {
    let rune_page_json = serde_json::to_string(&rune_page)?;
    conn.execute(
        "INSERT INTO rune_pages (champ_id, game_mode, spell1_id, spell2_id, page)
                  VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            champ_id as i64,
            game_mode,
            spells.0 as i64,
            spells.1 as i64,
            rune_page_json
        ],
    )?;
    Ok(())
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct MobaBuild {
    win_rate: f64,
    perks: MobaPerks,
    spells: Vec<String>,
    name: String,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct MobaPerks {
    ids: Vec<String>,
    style: String,
    sub_style: String,
}

fn row_to_data(row: &Row, champ_id: u64) -> Result<Vec<(RunePage, (u64, u64))>> {
    let spells: (i64, i64) = (row.get(0)?, row.get(1)?);
    let spells: (u64, u64) = (spells.0 as u64, spells.1 as u64);
    let page: String = row.get(2)?;
    let mut page: RunePage = serde_json::from_str(&page)?;
    println!("found spells & page: {:?} {:?}", spells, page);
    page.name = format!("{} (saved) {}", CHAMPIONS[&champ_id].to_string(), MARKER);
    Ok(vec![(page, spells)])
}

fn get_local_info(
    conn: &Connection,
    champ_id: u64,
    game_mode: &str,
) -> Result<Vec<(RunePage, (u64, u64))>> {
    let mut stmt = conn.prepare(
        "select spell1_id, spell2_id, page from rune_pages where champ_id = ?1 and game_mode = ?2",
    )?;
    let mut rows = stmt.query(params![champ_id as i64, game_mode])?;
    if let Some(row) = rows.next()? {
        return row_to_data(row, champ_id);
    }

    println!("couldn't find anything for champ and mode, trying just champ");
    let mut stmt =
        conn.prepare("select spell1_id, spell2_id, page from rune_pages where champ_id = ?1")?;
    let mut rows = stmt.query(params![champ_id as i64])?;
    if let Some(row) = rows.next()? {
        return row_to_data(row, champ_id);
    }
    // Just return an empty vec, so the next bit of code can add to it whether we found anything or not.
    Ok(Vec::new())
}

#[cached(time = 64800, result = true)]
fn get_mobalytics_info(champ_id: u64) -> Result<Vec<(RunePage, (u64, u64))>> {
    let mut name = CHAMPIONS[&champ_id].to_string();
    name.make_ascii_lowercase();
    name = name
        .chars()
        .filter(|c| match c {
            'a'..='z' => true,
            _ => false,
        })
        .collect();

    if name == "nunuwillump" {
        name.truncate(4);
    }

    let url = format!(
        "https://api.mobalytics.gg/lol/champions/v1/meta?name={}",
        name
    );
    println!("fetching {}", url);
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(4))
        .connect_timeout(Duration::from_secs(2))
        .build()?;
    let json = client.get(&url).send()?.text()?;
    let mut json: serde_json::Value = serde_json::from_str(&json)?;
    let mut all_builds = Vec::<MobaBuild>::new();
    let roles = json["data"]["roles"].as_array_mut();
    if let Some(roles) = roles {
        for role in roles {
            let mut builds: Vec<MobaBuild> = serde_json::from_value(role["builds"].to_owned())?;
            all_builds.append(&mut builds);
        }
    }
    // make sure the unwrap() with partial_cmp() doesn't fail
    for build in &mut all_builds {
        if build.win_rate.is_nan() {
            build.win_rate = 0.0;
        }
    }
    all_builds.sort_unstable_by(|a, b| b.win_rate.partial_cmp(&a.win_rate).unwrap());
    let all_builds = all_builds
        .into_iter()
        .map(|build| {
            let page = RunePage {
                name: format!("{} (mobalytics) {}", build.name, MARKER),
                primary_style_id: build.perks.style.parse()?,
                sub_style_id: build.perks.sub_style.parse()?,
                selected_perk_ids: build
                    .perks
                    .ids
                    .iter()
                    .filter_map(|s| s.parse().ok())
                    .collect(),
                ..Default::default()
            };
            let spell1_id: u64 = build.spells[0].parse()?;
            let spell2_id: u64 = build.spells[1].parse()?;
            Ok((page, (spell1_id, spell2_id)))
        })
        .collect();
    all_builds
}

fn delete_page(lcuclient: &LCUClient, page: &RunePage) -> Result<()> {
    lcuclient.delete(&format!("/lol-perks/v1/pages/{}", page.id))?;
    Ok(())
}

fn check_or_make_space(lcuclient: &LCUClient) -> Result<usize> {
    let pages = lcuclient.get("/lol-perks/v1/pages")?.text()?;
    let pages: Vec<RunePage> = serde_json::from_str(&pages)?;
    let pages: Vec<RunePage> = pages.into_iter().filter(|page| page.is_deletable).collect();

    let mut pages = pages
        .into_iter()
        .filter_map(|page| {
            if page.name.contains(MARKER) {
                match delete_page(&lcuclient, &page) {
                    Ok(_) => None,
                    Err(e) => Some(Err(e)),
                }
            } else {
                Some(Ok(page))
            }
        })
        .collect::<Result<Vec<RunePage>>>()?;
    let max_pages = lcuclient.get("/lol-perks/v1/inventory")?.text()?;
    let max_pages = serde_json::from_str::<serde_json::Value>(&max_pages)?["ownedPageCount"]
        .as_u64()
        .unwrap() as usize;
    let available_space = max_pages - pages.len();
    if available_space == 0 {
        println!("at max pages, deleting oldest");
        pages.sort_unstable_by(|a, b| a.last_modified.cmp(&b.last_modified));
        delete_page(&lcuclient, pages.first().context("No pages to delete?")?)?;
        return Ok(1);
    }
    Ok(available_space)
}

fn set_rune_page(lcuclient: &LCUClient, page: &RunePage) -> Result<()> {
    let new_page = lcuclient.post("/lol-perks/v1/pages", "{}")?.text()?;
    let new_page: RunePage = serde_json::from_str(&new_page)?;
    println!("created page, id: {}", new_page.id);

    println!("making page {} with name: {}", new_page.id, page.name);
    let put = lcuclient.put(
        &format!("/lol-perks/v1/pages/{}", new_page.id),
        serde_json::to_string(&page)?,
    )?;
    if put.status() != 201 {
        println!("{:?}", put.text()?);
        return Err(anyhow!("rune page creation was not a 201"));
    }
    println!("{:?}", put.text()?);
    Ok(())
}

fn setup_runes_and_spells(
    lcuclient: &LCUClient,
    conn: &Connection,
    champ_id: u64,
    game_mode: &str,
) -> Result<()> {
    let mut available_space = check_or_make_space(&lcuclient)?;

    let mut runes_and_spells = get_local_info(&conn, champ_id, game_mode)?;
    println!("after local, num pages: {}", runes_and_spells.len());
    println!("looking up on mobalytics");
    if let Ok(mut mobalytics) = get_mobalytics_info(champ_id) {
        runes_and_spells.append(&mut mobalytics);
    }
    println!("after mobalytics, num pages: {}", runes_and_spells.len());
    for (runes, _) in &mut runes_and_spells {
        runes.selected_perk_ids.sort();
    }

    // If runes and spells are the same, only keep one
    let mut runes_and_spells: Vec<(RunePage, (u64, u64))> = runes_and_spells
        .into_iter()
        .fold(Vec::new(), |mut acc, (runes, spells)| {
            if acc.iter().find(|&(previous_runes, previous_spells)| {
                //println!("comparing \n{:?}\nwith\n{:?}\n", (&runes, spells), (previous_runes, previous_spells));
                spells == *previous_spells
                && runes.primary_style_id == previous_runes.primary_style_id
                && runes.sub_style_id == previous_runes.sub_style_id
                && runes.selected_perk_ids == previous_runes.selected_perk_ids
            }).is_none() {
                acc.push((runes, spells));
            } else {
                println!("found duplicate");
            }
            acc
        });
    println!("after deduplication, num pages: {}", runes_and_spells.len());

    let len = runes_and_spells.len();
    if len > available_space {
        println!(
            "Have {} pages, but there's only room for {}",
            len, available_space
        );
        runes_and_spells.drain(available_space..);
    }
    println!("after size adjustment, num pages: {}", runes_and_spells.len());

    println!("num pages: {}", runes_and_spells.len());

    // Reverse so highest winrate is installed last (and thus is active at the end)
    runes_and_spells.reverse();

    for (index, (page, mut spells)) in runes_and_spells.into_iter().enumerate() {
        if available_space == 0 {
            break;
        }
        set_rune_page(lcuclient, &page)?;
        if index == 0 {
            if spells.0 == 4 {
                spells.0 = spells.1;
                spells.1 = 4;
            }
            let _ = lcuclient.patch(
                "/lol-champ-select/v1/session/my-selection",
                format!(
                    "{{ \"spell1Id\": {}, \"spell2Id\": {} }}",
                    spells.0, spells.1
                ),
            )?;
        }
        available_space -= 1;
    }
    Ok(())
}

fn main() -> Result<()> {
    //    let page = get_mobalytics_page(875);
    //    todo!();

    let conn = setup_sqlite()?;

    let mut stmt =
        conn.prepare("select champ_id, game_mode, spell1_id, spell2_id, page from rune_pages")?;
    let mut rows = stmt.query(NO_PARAMS)?;

    let mut num: usize = 0;
    while rows.next()?.is_some() {
        num += 1;
    }

    println!("stored pages: {}", num);
    loop {
        let _ = run_event_loop(&conn);
    }
}

fn run_event_loop(conn: &Connection) -> Result<()> {
    let mut game_mode: Option<String> = None;
    let mut rune_page: Option<RunePage> = None;
    let mut champ_id: Option<u64> = None;
    let mut phase: Option<GamePhase> = None;
    let mut spells: Option<(u64, u64)> = None;

    let lcuclient = LCUClient::new()?;
    if clean_pages(&lcuclient).is_err() {
        println!("LCU not returning data, sleeping...");
        thread::sleep(time::Duration::from_secs(2));
        return Ok(());
    }

    let (champ_sender, champ_receiver) = channel();
    let (spells_sender, spells_receiver) = channel();
    let mut ws = LCUWebSocket::new();
    ws.subscribe(
        "OnJsonApiEvent_lol-champ-select_v1_session".to_string(),
        move |json| {
            /* convert to string and back so we fully own the data, since ::from_value doesn't take
             * a reference. */
            let session = serde_json::to_string(&json["data"])?;
            let session: Result<SelectSession, _> = serde_json::from_str(&session);
            if let Ok(session) = session {
                let local_player_cell_id = session.local_player_cell_id;
                let me = session
                    .my_team
                    .into_iter()
                    .find(|player| player.cell_id == local_player_cell_id);
                if let Some(me) = me {
                    let champ_id = if me.champion_id != 0 {
                        me.champion_id
                    } else {
                        me.champion_pick_intent
                    };
                    if champ_id != 0 {
                        champ_sender.send(champ_id)?;
                    }
                    if me.spell1_id != 0 && me.spell2_id != 0 {
                        spells_sender.send((me.spell1_id, me.spell2_id))?
                    }
                }
            }
            Ok(())
        },
    );

    let (gm_sender, gm_receiver) = channel();
    let (phase_sender, phase_receiver) = channel();
    ws.subscribe(
        "OnJsonApiEvent_lol-gameflow_v1_session".to_string(),
        move |json| {
            //println!("{}", serde_json::to_string_pretty(&json)?);
            let gm = json["data"]["gameData"]["queue"]["gameMode"]
                .as_str()
                .context("no game mode")?
                .to_string();
            gm_sender.send(gm)?;
            let phase = if let Some(phase) = json["data"]["phase"].as_str() {
                match phase {
                    "Lobby" => Some(GamePhase::Lobby),
                    "Matchmaking" => Some(GamePhase::Matchmaking),
                    "ReadyCheck" => Some(GamePhase::ReadyCheck),
                    "ChampSelect" => Some(GamePhase::ChampSelect),
                    "GameStart" => Some(GamePhase::GameStart),
                    "InProgress" => Some(GamePhase::InProgress),
                    "WaitingForStats" => Some(GamePhase::WaitingForStats),
                    "PreEndOfGame" => Some(GamePhase::PreEndOfGame),
                    "EndOfGame" => Some(GamePhase::EndOfGame),
                    _ => None,
                }
            } else {
                None
            };
            phase_sender.send(phase)?;
            Ok(())
        },
    );

    let (runes_sender, runes_receiver) = channel();
    ws.subscribe(
        "OnJsonApiEvent_lol-perks_v1_currentpage".to_string(),
        move |json| {
            /* convert to string and back so we fully own the data, since ::from_value doesn't take
             * a reference. */
            let rune_page = serde_json::to_string(&json["data"]);
            if let Ok(rune_page) = rune_page {
                let rune_page: RunePage = serde_json::from_str(&rune_page)?;
                runes_sender.send(Some(rune_page))?;
            } else {
                runes_sender.send(None)?;
            }
            Ok(())
        },
    );

    /*
        let all_file_sender = sender.clone();
        ws.subscribe("OnJsonApiEvent".to_string(), move |json| {
            all_file_sender.send(format!("{}", serde_json::to_string_pretty(&json)?))?;
            println!("{}", json["uri"].as_str()?);
            LCUWSResult::Continue
        });
    */

    while let Ok(()) = ws.dispatch() {
        while let Ok(new_gm) = gm_receiver.try_recv() {
            match game_mode {
                None => {
                    println!("Game mode: {}", new_gm);
                    game_mode = Some(new_gm);
                }
                Some(prevgm) if prevgm != new_gm => {
                    println!("Game mode: {}", new_gm);
                    game_mode = Some(new_gm);
                }
                Some(_) => (),
            }
        }

        while let Ok(runes) = runes_receiver.try_recv() {
            let prev_rune_name = &rune_page.as_ref().map(|r| r.name.clone());
            rune_page = runes;
            if prev_rune_name != &rune_page.as_ref().map(|r| r.name.clone()) {
                if let Some(runes) = &rune_page {
                    println!("Rune page: {:?}", runes.name);
                } else {
                    println!("No rune page");
                }
            }
        }

        while let Ok(cid) = champ_receiver.try_recv() {
            let prev_champ_id = champ_id;
            champ_id = Some(cid);
            if prev_champ_id != champ_id {
                println!("Champ ID: {:?}", cid);
                if let Some(game_mode) = &game_mode {
                    println!("setup runes");
                    setup_runes_and_spells(&lcuclient, &conn, cid, &game_mode)?;
                } else {
                    println!("got champ_id, but no qid, using UNKNOWN");
                    setup_runes_and_spells(&lcuclient, &conn, cid, &"UNKNOWN")?;
                }
            }
        }

        while let Ok(sp) = spells_receiver.try_recv() {
            let prev_spells = spells;
            spells = Some(sp);
            if prev_spells != spells {
                println!("Spells: {} {}", sp.0, sp.1);
            }
        }

        while let Ok(p) = phase_receiver.try_recv() {
            let prev_phase = phase;
            phase = p;
            if prev_phase != phase {
                if let Some(p) = phase {
                    if p == GamePhase::GameStart {
                        let rune_page = lcuclient.get("/lol-perks/v1/currentpage")?.text()?;
                        let rune_page: RunePage = serde_json::from_str(&rune_page)?;
                        if let (Some(champ_id), Some(spells), Some(game_mode)) =
                            (champ_id, spells, &game_mode)
                        {
                            println!("Saving rune page");
                            save_rune_page(&conn, champ_id, spells, &game_mode, &rune_page)?;
                        } else {
                            if champ_id == None {
                                println!("Missing champ_id");
                            }
                            if spells == None {
                                println!("Missing spells");
                            }
                            if game_mode == None {
                                println!("Missing game mode");
                            }
                            println!("Missing champ_id/spells/game mode/runes at game start");
                        }
                    }
                    println!("Phase: {:?}", p);
                } else {
                    println!("No phase");
                }
            }
        }
        //println!("while loop");
    }
    Ok(())
}

fn clean_pages(lcuclient: &LCUClient) -> Result<()> {
    let pages = lcuclient.get("/lol-perks/v1/pages")?.text()?;
    let pages: Vec<RunePage> = serde_json::from_str(&pages)?;
    if pages.is_empty() {
        return Err(anyhow!("No pages yet"));
    } else {
        println!("{} rune pages in client", pages.len());
    }
    let mut pages: Vec<RunePage> = pages.into_iter().filter(|page| page.is_deletable).collect();

    pages.sort_unstable_by(|a, b| {
        let ord = a.name.cmp(&b.name);
        if ord == Ordering::Equal {
            a.last_modified.cmp(&b.last_modified)
        } else {
            ord
        }
    });

    println!("all deletable pages:");
    for page in pages.iter() {
        println!(
            "  {} [id:{}] [lm:{}]",
            page.name, page.id, page.last_modified
        );
    }

    let mut peekable = pages.into_iter().peekable();
    let mut pages_to_delete = Vec::new();
    while let Some(page) = peekable.next() {
        if let Some(next) = peekable.peek() {
            if next.name == page.name || !page.is_valid {
                pages_to_delete.push(page);
            }
        }
    }

    // XXX: make sure there's an empty slot 
    if pages_to_delete.is_empty() {
        println!("deleting:");
        for page in pages_to_delete.into_iter() {
            println!(
                "  {} [id:{}] [lm:{}]",
                page.name, page.id, page.last_modified
            );
            delete_page(&lcuclient, &page)?;
        }
    } else {
        println!("nothing to clean");
    }
    Ok(())
}
