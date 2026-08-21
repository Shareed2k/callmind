use callmind_core::Language;
use callmind_transcript::Transcript;

/// Smart language-aware conversation summarizer and topic classifier.
pub struct ConversationSummarizer;

pub struct HeuristicSummary {
    pub title: String,
    pub summary: String,
    pub reason: Option<String>,
    pub resolution: Option<String>,
    pub intent: Option<String>,
    pub topics: Vec<String>,
}

impl ConversationSummarizer {
    /// Generate a coherent structured summary and topic breakdown from transcript text.
    #[must_use]
    pub fn summarize(transcript: &Transcript, primary_lang: &Language) -> HeuristicSummary {
        let full_text = transcript.full_text().to_lowercase();
        let num_segments = transcript.segments.len();

        match primary_lang {
            Language::Russian => Self::summarize_russian(&full_text, transcript, num_segments),
            Language::Hebrew => Self::summarize_hebrew(&full_text, transcript, num_segments),
            _ => Self::summarize_english(&full_text, transcript, num_segments),
        }
    }

    fn summarize_russian(
        text: &str,
        _transcript: &Transcript,
        num_segments: usize,
    ) -> HeuristicSummary {
        if text.contains("где ты")
            || text.contains("я здесь")
            || text.contains("автобус")
            || text.contains("далеко")
            || text.contains("подъезжаю")
        {
            HeuristicSummary {
                title: "Координация встречи и местоположения".to_string(),
                summary: "Собеседники созваниваются, чтобы уточнить текущее местоположение, маршрут и скоординировать встречу.".to_string(),
                reason: Some("Уточнение местоположения и времени встречи".to_string()),
                resolution: Some("Участники сориентировались и договорились о встрече".to_string()),
                intent: Some("Координация встречи".to_string()),
                topics: vec!["Встреча".into(), "Локация".into(), "Маршрут".into()],
            }
        } else if text.contains("курьер")
            || text.contains("доставка")
            || text.contains("посылк")
            || text.contains("заказ")
        {
            HeuristicSummary {
                title: "Доставка и получение заказа".to_string(),
                summary: "Обсуждение деталей доставки, времени прибытия курьера и передачи заказа."
                    .to_string(),
                reason: Some("Координация передачи доставки курьером".to_string()),
                resolution: Some("Детали передачи согласованы".to_string()),
                intent: Some("Доставка".to_string()),
                topics: vec!["Доставка".into(), "Курьер".into(), "Заказ".into()],
            }
        } else if text.contains("не работает")
            || text.contains("ошибк")
            || text.contains("проблем")
            || text.contains("сломал")
        {
            HeuristicSummary {
                title: "Технические неполадки и поддержка".to_string(),
                summary: "Обсуждение возникшей технической неисправности и шагов по её диагностике или устранению.".to_string(),
                reason: Some("Обращение по поводу неисправности".to_string()),
                resolution: Some("Проблема зафиксирована для решения".to_string()),
                intent: Some("Техподдержка".to_string()),
                topics: vec!["Техподдержка".into(), "Неисправность".into()],
            }
        } else if text.contains("оплат")
            || text.contains("счет")
            || text.contains("деньг")
            || text.contains("банк")
            || text.contains("чек")
        {
            HeuristicSummary {
                title: "Финансовые вопросы и взаиморасчеты".to_string(),
                summary: "Обсуждение выставленных счетов, деталей оплаты или финансовых операций."
                    .to_string(),
                reason: Some("Уточнение финансовых вопросов".to_string()),
                resolution: Some("Условия оплаты согласованы".to_string()),
                intent: Some("Финансы".to_string()),
                topics: vec!["Оплата".into(), "Счета".into(), "Финансы".into()],
            }
        } else {
            let desc = if num_segments <= 3 {
                "Короткий телефонный разговор между собеседниками для оперативной связи."
            } else {
                "Телефонный разговор, в ходе которого собеседники обсудили текущие вопросы и согласовали действия."
            };
            HeuristicSummary {
                title: "Телефонный разговор".to_string(),
                summary: desc.to_string(),
                reason: Some("Обсуждение текущих вопросов".to_string()),
                resolution: Some("Вопросы обсуждены".to_string()),
                intent: Some("Личная беседа".to_string()),
                topics: vec!["Общение".into(), "Текущие вопросы".into()],
            }
        }
    }

    fn summarize_hebrew(
        text: &str,
        _transcript: &Transcript,
        num_segments: usize,
    ) -> HeuristicSummary {
        if text.contains("שליח")
            || text.contains("משלוח")
            || text.contains("חבילה")
            || text.contains("הזמנה")
        {
            HeuristicSummary {
                title: "תיאום משלוח והגעת שליח".to_string(),
                summary: "שיחה בנושא תיאום קבלת משלוח, כתובת ומועד הגעת השליח ללקוח.".to_string(),
                reason: Some("תיאום מסירת משלוח".to_string()),
                resolution: Some("פרטי המסירה תואמו בהצלחה".to_string()),
                intent: Some("משלוחים".to_string()),
                topics: vec!["משלוח".into(), "שליח".into(), "חבילה".into()],
            }
        } else if text.contains("איפה אתה")
            || text.contains("אני פה")
            || text.contains("הגעתי")
            || text.contains("בדרך")
        {
            HeuristicSummary {
                title: "תיאום מיקום ומפגש".to_string(),
                summary: "שיחה קצרה בין הדוברים לבירור מיקום מדויק ותיאום נקודת מפגש.".to_string(),
                reason: Some("בירור מיקום והגעה".to_string()),
                resolution: Some("נקודת המפגש סוכמה".to_string()),
                intent: Some("תיאום הגעה".to_string()),
                topics: vec!["מפגש".into(), "מיקום".into()],
            }
        } else if text.contains("תקלה")
            || text.contains("לא עובד")
            || text.contains("בעיה")
            || text.contains("שירות")
        {
            HeuristicSummary {
                title: "בירור תקלה ותמיכה טכנית".to_string(),
                summary: "פנייה בנוגע לתקלה טכנית או בעיה תפעולית הדורשת בדיקה וטיפול.".to_string(),
                reason: Some("דיווח על תקלה".to_string()),
                resolution: Some("הפנייה נרשמה להמשך טיפול".to_string()),
                intent: Some("תמיכה טכנית".to_string()),
                topics: vec!["תמיכה".into(), "תקלה טכנית".into()],
            }
        } else if text.contains("תשלום")
            || text.contains("חשבונית")
            || text.contains("אשראי")
            || text.contains("חיוב")
        {
            HeuristicSummary {
                title: "בירור תשלום וחשבוניות".to_string(),
                summary: "שיחה לבירור פרטי חיוב, אמצעי תשלום או הפקת חשבונית.".to_string(),
                reason: Some("בירור נושאי כספים וחיובים".to_string()),
                resolution: Some("פרטי החיוב הובהרו".to_string()),
                intent: Some("כספים ותשלומים".to_string()),
                topics: vec!["תשלום".into(), "חשבונית".into()],
            }
        } else {
            let desc = if num_segments <= 3 {
                "שיחה קצרה בין הדוברים לבירור ותיאום מהיר."
            } else {
                "שיחה שבה הדוברים דנו בנושאים שונים ותיאמו את המשך ההתנהלות."
            };
            HeuristicSummary {
                title: "שיחת טלפון".to_string(),
                summary: desc.to_string(),
                reason: Some("בירור נושאים שוטפים".to_string()),
                resolution: Some("הנושאים סוכמו".to_string()),
                intent: Some("שיחה כללית".to_string()),
                topics: vec!["שיחה".into(), "תיאום".into()],
            }
        }
    }

    fn summarize_english(
        text: &str,
        _transcript: &Transcript,
        num_segments: usize,
    ) -> HeuristicSummary {
        if text.contains("where are you")
            || text.contains("i am here")
            || text.contains("arrived")
            || text.contains("on my way")
        {
            HeuristicSummary {
                title: "Meeting & Location Coordination".to_string(),
                summary: "The speakers checked in to clarify current locations, route, and coordinate their arrival.".to_string(),
                reason: Some("Arrival and location coordination".to_string()),
                resolution: Some("Meeting location agreed upon".to_string()),
                intent: Some("Coordination".to_string()),
                topics: vec!["Meeting".into(), "Location".into()],
            }
        } else if text.contains("delivery")
            || text.contains("courier")
            || text.contains("package")
            || text.contains("order")
        {
            HeuristicSummary {
                title: "Package Delivery & Coordination".to_string(),
                summary: "Discussion regarding shipment status, delivery address, and courier arrival time.".to_string(),
                reason: Some("Package delivery coordination".to_string()),
                resolution: Some("Delivery details confirmed".to_string()),
                intent: Some("Delivery".to_string()),
                topics: vec!["Delivery".into(), "Package".into()],
            }
        } else {
            let desc = if num_segments <= 3 {
                "Brief conversation between participants for quick coordination."
            } else {
                "Discussion between participants regarding ongoing matters and next steps."
            };
            HeuristicSummary {
                title: "Recorded Conversation".to_string(),
                summary: desc.to_string(),
                reason: Some("General discussion".to_string()),
                resolution: Some("Matters discussed".to_string()),
                intent: Some("General".to_string()),
                topics: vec!["Discussion".into()],
            }
        }
    }
}
