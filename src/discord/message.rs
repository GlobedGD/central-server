use std::borrow::Cow;

pub use super::serenity::CreateEmbed;

type Str<'a> = Cow<'a, str>;

// #[derive(Default, Clone, Debug)]
// pub struct DiscordAuthor<'a> {
//     pub name: Str<'a>,
//     pub icon_url: Option<Str<'a>>,
// }

// impl<'a> DiscordAuthor<'a> {
//     pub fn new() -> Self {
//         Self::default()
//     }

//     pub fn name(mut self, name: impl Into<Str<'a>>) -> Self {
//         self.name = name.into();
//         self
//     }

//     pub fn icon_url(mut self, url: impl Into<Str<'a>>) -> Self {
//         self.icon_url = Some(url.into());
//         self
//     }
// }

// #[derive(Default, Clone, Debug)]
// pub struct DiscordFooter<'a> {
//     pub text: Str<'a>,
//     pub icon_url: Option<Str<'a>>,
// }

// impl<'a> DiscordFooter<'a> {
//     pub fn new() -> Self {
//         Self::default()
//     }

//     pub fn text(mut self, text: impl Into<Str<'a>>) -> Self {
//         self.text = text.into();
//         self
//     }

//     pub fn icon_url(mut self, url: impl Into<Str<'a>>) -> Self {
//         self.icon_url = Some(url.into());
//         self
//     }
// }

// #[derive(Default, Clone, Debug)]
// pub struct DiscordEmbedField<'a> {
//     pub name: Str<'a>,
//     pub value: Str<'a>,
//     pub inline: bool,
// }

// #[derive(Default, Clone, Debug)]
// pub struct DiscordThumbnail<'a> {
//     pub url: Str<'a>,
// }

// impl<'a> DiscordThumbnail<'a> {
//     pub fn new() -> Self {
//         Self::default()
//     }
// }

// #[derive(Default, Clone, Debug)]
// pub struct DiscordEmbed<'a> {
//     pub title: Str<'a>,
//     pub author: Option<DiscordAuthor<'a>>,
//     pub description: Option<Str<'a>>,
//     pub footer: Option<DiscordFooter<'a>>,
//     pub fields: Vec<DiscordEmbedField<'a>>,
//     pub thumbnail: Option<DiscordThumbnail<'a>>,
// }

// impl<'a> DiscordEmbed<'a> {
//     pub fn new() -> Self {
//         Self::default()
//     }

//     pub fn title(mut self, title: impl Into<Str<'a>>) -> Self {
//         self.title = title.into();
//         self
//     }

//     pub fn author(mut self, author: DiscordAuthor<'a>) -> Self {
//         self.author = Some(author);
//         self
//     }

//     pub fn description(mut self, desc: impl Into<Str<'a>>) -> Self {
//         self.description = Some(desc.into());
//         self
//     }

//     pub fn footer(mut self, footer: DiscordFooter<'a>) -> Self {
//         self.footer = Some(footer);
//         self
//     }

//     pub fn add_field(mut self, name: impl Into<Str<'a>>, value: impl Into<Str<'a>>) -> Self {
//         self.fields.push(DiscordEmbedField {
//             name: name.into(),
//             value: value.into(),
//             inline: false,
//         });
//         self
//     }

//     pub fn add_inline_field(mut self, name: impl Into<Str<'a>>, value: impl Into<Str<'a>>) -> Self {
//         self.fields.push(DiscordEmbedField {
//             name: name.into(),
//             value: value.into(),
//             inline: false,
//         });
//         self
//     }

//     pub fn thumbnail(mut self, url: impl Into<Str<'a>>) -> Self {
//         self.thumbnail = Some(DiscordThumbnail { url: url.into() });
//         self
//     }
// }

// #[derive(Default, Clone, Debug)]
// pub struct DiscordMessage<'a> {
//     pub content: Option<Str<'a>>,
//     pub embeds: Vec<DiscordEmbed<'a>>,
// }

// impl<'a> DiscordMessage<'a> {
//     pub fn new() -> Self {
//         Self::default()
//     }

//     pub fn content(mut self, content: Str<'a>) -> Self {
//         self.content = Some(content);
//         self
//     }

//     pub fn add_embed(mut self, embed: DiscordEmbed<'a>) -> Self {
//         self.embeds.push(embed);
//         self
//     }
// }

#[derive(Default, Clone, Debug)]
pub struct DiscordMessage<'a> {
    pub content: Option<Str<'a>>,
    pub embeds: Vec<CreateEmbed>,
}

impl<'a> DiscordMessage<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn content(mut self, content: impl Into<Str<'a>>) -> Self {
        self.content = Some(content.into());
        self
    }

    pub fn add_embed(mut self, embed: CreateEmbed) -> Self {
        self.embeds.push(embed);
        self
    }
}
