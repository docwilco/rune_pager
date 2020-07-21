use std::env;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

fn main() -> std::io::Result<()> {
    let infile = std::fs::File::open("champion.json").unwrap();
    let json: serde_json::Value = serde_json::from_reader(infile).unwrap();
    let champions = json["data"].as_object().unwrap();

    let mut map = phf_codegen::Map::<u64>::new();

    for (_, champ) in champions {
        //println!("{:?}", champ["name"].as_str().unwrap());
        map.entry(
            champ["key"].as_str().unwrap().parse().unwrap(),
            &format!("{:?}", champ["name"].as_str().unwrap()),
        );
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
