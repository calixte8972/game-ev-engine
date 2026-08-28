use crate::{Card, DEFAULT_DECKS, Shoe, ShoeError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardSource {
    Consumed,
    Remaining,
}

#[derive(Debug, PartialEq, Eq)]
pub struct AnalyzeInput {
    pub source: CardSource,
    pub cards: Vec<Card>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Analyze(AnalyzeInput),
}

#[derive(Debug, PartialEq, Eq)]
pub enum CliError {
    MissingCommand,
    UnknownCommand(String),
    MissingCardSource,
    UnknownCardSource(String),
    InvalidCard(String),
    Shoe(ShoeError),
    UnsupportedCardSource(CardSource),
}
impl From<ShoeError> for CliError {
    fn from(error: ShoeError) -> Self {
        Self::Shoe(error)
    }
}
pub fn build_shoe(input: &AnalyzeInput) -> Result<Shoe, CliError> {
    match input.source {
        CardSource::Consumed => {
            let mut shoe = Shoe::default();

            shoe.remove_many(&input.cards)?;

            Ok(shoe)
        }
        CardSource::Remaining => Ok(Shoe::from_remaining(DEFAULT_DECKS, &input.cards)?),
    }
}
pub fn parse_args(args: &[String]) -> Result<Command, CliError> {
    let command = args.first().ok_or(CliError::MissingCommand)?;

    if command != "analyze" {
        return Err(CliError::UnknownCommand(command.clone()));
    }

    let source_text = args.get(1).ok_or(CliError::MissingCardSource)?;

    let source = match source_text.as_str() {
        "consumed" => CardSource::Consumed,
        "remaining" => CardSource::Remaining,
        other => return Err(CliError::UnknownCardSource(other.to_owned())),
    };

    let mut cards = Vec::new();

    for card_text in &args[2..] {
        let card = card_text
            .parse::<Card>()
            .map_err(|_| CliError::InvalidCard(card_text.clone()))?;

        cards.push(card);
    }

    Ok(Command::Analyze(AnalyzeInput { source, cards }))
}
