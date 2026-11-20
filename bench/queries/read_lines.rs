let file = File::open(path)?;
let reader = BufReader::new(file);
for line in reader.lines() {
    let line = line?;
}
