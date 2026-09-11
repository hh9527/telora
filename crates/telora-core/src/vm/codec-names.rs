fn lower_camel_case(name: &str) -> String {
    let mut output = String::with_capacity(name.len());
    let mut uppercase = false;
    for (index, character) in name.chars().enumerate() {
        if character == '_' {
            uppercase = true;
        } else if uppercase {
            output.extend(character.to_uppercase());
            uppercase = false;
        } else if index == 0 {
            output.extend(character.to_lowercase());
        } else {
            output.push(character);
        }
    }
    output
}
