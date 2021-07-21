use std::env;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use reqwest::blocking::get;

fn main() -> anyhow::Result<()> {
    let infile = get("https://raw.communitydragon.org/latest/plugins/rcp-be-lol-game-data/global/default/v1/champion-summary.json")?;
    let champions: Vec<serde_json::Value> = serde_json::from_reader(infile)?;

    let mut map = phf_codegen::Map::<u64>::new();

    for champ in champions {
        let key = champ["id"].as_i64().unwrap();
        if key < 0 {
            continue;
        }
        let key = key as u64;
        let value = &format!("{:?}", champ["name"].as_str().unwrap());
        map.entry(key, value);
    }
    let outfile = Path::new(&env::var("OUT_DIR").unwrap()).join("champions.rs");
    let mut outfile = BufWriter::new(File::create(&outfile).unwrap());

    writeln!(
        &mut outfile,
        "static CHAMPIONS: phf::Map<u64, &str> = \n{};\n",
        map.build()
    )
    .unwrap();

    Ok(())
}
