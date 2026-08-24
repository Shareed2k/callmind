use callmind_core::Language;

/// System prompt for conversational intelligence analysis across Hebrew, Russian, and English.
pub const CONVERSATION_ANALYSIS_SYSTEM_PROMPT: &str = r#"
You are a senior conversation intelligence analyst.
Report only what the transcript says. Extract these when, and only when, they are
actually spoken -- most conversations contain some of them and not others:
- WHO: names, relationships or roles, if stated.
- WHAT: what is happening, what each speaker asked for.
- WHERE: locations, addresses, buildings, floors or room numbers, if mentioned.
- WHEN: dates, times, deadlines, if mentioned.
- OUTCOME: the conclusion or agreement, if one was reached.

A category with nothing said about it is left empty. Do not fill it to satisfy the
list -- an empty field is a correct answer, and a plausible guess is a wrong one.

Never invent, infer, or complete facts that are not explicitly present in the transcript.
Do not introduce example names (such as John or Jane), locations, floors, dates, amounts,
roles, promises, or outcomes unless the transcript states them. When information is absent,
use null, an empty array, or a short statement that the detail was not provided.
Include concrete names, places, and facts only when they are supported by transcript text.
Output strictly valid JSON matching the schema.
"#;

/// Build the structured prompt for a full call analysis with primary language adaptation.
#[must_use]
pub fn build_language_aware_analysis_prompt(
    transcript_text: &str,
    organization_name: &str,
    language: &Language,
) -> String {
    if *language == Language::Russian {
        return format!(
            r#"
Проанализируй следующий записанный разговор ({organization_name}).
ВАЖНОЕ ПРАВИЛО: Разговор на русском языке. Все значения в JSON (title, summary, reason, resolution, customer_intent, topics, key_facts, action_items) пиши СТРОГО НА РУССКОМ ЯЗЫКЕ.
ГЛАВНОЕ ПРАВИЛО: пиши ТОЛЬКО то, что прозвучало в записи. Имена, места, этажи, комнаты, даты, суммы, роли, договоренности и результаты указывай лишь тогда, когда они названы прямо. Ничего не добавляй, не додумывай и не обобщай. Если детали нет — используй null, пустой список или прямо напиши, что она не названа. Пустое поле — правильный ответ, правдоподобная догадка — неправильный.

Транскрипт:
---
{transcript_text}
---

Сформируй подробный JSON-отчет со следующими полями:
1. title: краткий заголовок (3-7 слов) на русском языке по теме разговора, только из того, что в нём прозвучало.
2. summary: краткое и ясное резюме: о чём договорились, какие действия требуются, имена, даты и все важные детали, упомянутые в разговоре.
3. reason: конкретная причина или цель звонка на русском языке.
4. resolution: точный результат, договоренность или принятое решение на русском языке.
5. resolved: true если разговор завершился решением/согласием, false если вопрос остался нерешенным.
6. customer_intent: категория намерения на русском языке (например, "координация", "личный", "договоренность", "поддержка", "заказ").
7. topics: список конкретных тем на русском языке.
8. key_facts: список из 2-5 конкретных фактов из разговора на русском языке (кто где находится, что сделано, имена, детали).
9. action_items: список задач или договоренностей (с полями text на русском языке, owner, deadline).
10. entities: список сущностей с полями entity_type ("person", "location", "floor", "phone", "date_time") и value на русском языке.
11. sentiment_score: общая тональность от -1.0 до 1.0.
12. risks: риски или недопонимания на русском языке.
13. promises: явные обещания или соглашения на русском языке.
"#
        );
    }

    if *language == Language::Hebrew {
        return format!(
            r#"
נתח את שיחת הטלפון המוקלטת הבאה ({organization_name}).
כלל קריטי: השיחה היא בעברית. כל ערכי הטקסט ב-JSON (כותרת, סיכום, סיבה, תוצאה, נושאים, עובדות מפתח ומשימות) חייבים להיכתב בעברית טבעית ומדויקת. שמור על שמות המפתחות ב-JSON באנגלית.
הכלל העיקרי: כתוב רק את מה שנאמר בהקלטה. שמות, מקומות, קומות, תאריכים, סכומים, תפקידים והסכמות -- רק כאשר הם נאמרים במפורש. אל תוסיף, אל תשלים ואל תכליל דבר. כאשר פרט חסר, השתמש ב-null, ברשימה ריקה או ציין שהפרט לא נאמר. שדה ריק הוא תשובה נכונה; ניחוש סביר הוא תשובה שגויה.

תמליל השיחה:
---
{transcript_text}
---

החזר אובייקט JSON תקני עם המפתחות הבאים:
1. title: כותרת ממוקדת בת 3-7 מילים בעברית המציינת את הנושא או המשתתפים.
2. summary: סיכום תמציתי וברור: מה סוכם, אילו פעולות נדרשות, שמות, תאריכים וכל פרט חשוב שהוזכר בשיחה.
3. reason: סיבת השיחה והמטרה העיקרית בעברית.
4. resolution: התוצאה וההסכמה שהושגה בעברית.
5. resolved: true אם הנושא נסגר/הוסכם, false אם לא הושלם.
6. customer_intent: כוונת השיחה בעברית (למשל: תיאום, אישי, שירות, בירור, קביעת תור).
7. topics: רשימת נושאים ספציפיים בעברית.
8. key_facts: רשימת 2-5 עובדות מפתח קונקרטיות מתוך השיחה בעברית.
9. action_items: רשימת משימות או התחייבויות (עם text בעברית, owner, deadline).
10. entities: ישויות שחולצו (שמות אנשים, מיקומים, קומות, טלפונים, סכומים).
11. sentiment_score: ציון סנטימנט בין -1.0 ל-+1.0.
12. risks: אי-הבנות או נושאים דחופים בעברית.
13. promises: הבטחות והסכמות מפורשות בעברית.
"#
        );
    }

    format!(
        r#"
Analyze the following recorded conversation ({organization_name}).
PRIMARY RULE: report only what was said in the recording. State names, places, floors, rooms, dates, amounts, roles, agreements and outcomes only where the transcript states them. Add nothing, infer nothing, generalise nothing. Where a detail is absent use null, an empty list, or say it was not provided. An empty field is a correct answer; a plausible guess is a wrong one.

Transcript:
---
{transcript_text}
---

Return a structured JSON object with:
1. title: specific, informative 3-8 word title capturing the concrete subject and participants.
2. summary: a concise, clear summary: what was agreed, what actions are required, names, dates, and every important detail mentioned in the conversation.
3. reason: specific purpose/motive of the call with concrete details.
4. resolution: the exact outcome, agreement, or next step concluded between the participants.
5. resolved: boolean indicating whether the matter discussed was completed or agreed upon.
6. customer_intent: specific category (e.g., "coordination", "personal", "status_update", "scheduling", "inquiry", "support", "billing").
7. topics: array of specific topics discussed.
8. key_facts: up to 5 bullet points, each stating something the transcript actually says. Fewer is correct if the conversation was short.
9. action_items: list of commitments or next steps with owner ("speaker_1", "speaker_2", or person name), text, deadline, and evidence_segments.
10. entities: list of extracted entities (person names, locations, rooms/floors, phone numbers, prices/amounts, dates/times).
11. sentiment_score: overall tone/sentiment from -1.0 (very negative) to +1.0 (very positive).
12. risks: list of any disagreements, urgent matters, or misunderstandings identified with evidence.
13. promises: explicit promises, agreements, or commitments made between speakers.
"#
    )
}

/// Build the structured prompt for a full call analysis.
#[must_use]
pub fn build_analysis_prompt(transcript_text: &str, organization_name: &str) -> String {
    build_language_aware_analysis_prompt(transcript_text, organization_name, &Language::Hebrew)
}

/// Prompt for compressing one window of a long transcript.
///
/// Used when a call will not fit the model's context window: each window is
/// summarised and the summaries take the transcript's place. The instruction is
/// deliberately the same "only what was said" rule as the analysis prompt, since
/// a fabrication introduced here would be laundered into the final analysis with
/// nothing left to check it against.
#[must_use]
pub fn build_window_compression_prompt(window: &str, language: &Language) -> String {
    let instruction = match language {
        Language::Hebrew => {
            "סכם את הקטע הבא מהשיחה בעברית, בקצרה. שמור שמות, מספרים, תאריכים, סכומים \
             והחלטות בדיוק כפי שנאמרו. אל תוסיף דבר שלא נאמר."
        }
        Language::Russian => {
            "Сожми следующий фрагмент разговора на русском языке, кратко. Сохрани \
             имена, числа, даты, суммы и договорённости точно как сказано. Не добавляй ничего, \
             чего не было."
        }
        _ => {
            "Summarise the following passage of the call, briefly. Keep names, numbers, dates, \
             amounts and decisions exactly as spoken. Add nothing that was not said."
        }
    };
    format!("{instruction}\n\n---\n{window}\n---")
}
