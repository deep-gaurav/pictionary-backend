use yaml_rust::YamlLoader;

pub fn getrandomword() -> String {
    let docs = std::fs::read_to_string("game_words.yaml").expect("Cant load yaml file");
    let docs = YamlLoader::load_from_str(&docs).expect("Not valid yamml");
    let doc1 = &docs[0];
    let mut words = doc1["pictionary"]["medium"]
        .as_vec()
        .expect("catchphrase easy not found").to_owned();
    words.append(&mut doc1["pictionary"]["hard"].as_vec().expect("hard pictionary not found").to_owned());

    use rand::seq::SliceRandom;
    let word = words.choose(&mut rand::thread_rng()).expect("No word");
    word.as_str().expect("Word not string").to_string()
}

pub fn getavatarcolor(name: &str) -> String {
    let firstchar = name.chars().next().unwrap_or('.');
    let hue = (random(firstchar as u32) * 360_f64) as u32;
    format!("hsl({},100%,50%)", hue)
}

pub fn random(seed: u32) -> f64 {
    let x = ((seed + 1) as f64).sin() * 10000_f64;
    x - x.floor()
}
