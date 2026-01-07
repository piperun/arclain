#[cfg(test)]
mod tests {
    use crate::dlsite::parse_html_response;

    #[test]
    fn test_parse_work_outline() {
        let html = r##"
        <html>
        <body>
            <h1 id="work_name">Test Title</h1>
            <div id="work_right">
                <span class="maker_name"><a href="#">Test Circle</a></span>
            </div>
            
            <table id="work_maker">
                <tr><th>Brand</th><td>Test Brand</td></tr>
                <tr><th>Publisher</th><td>Test Publisher</td></tr>
            </table>

            <table id="work_outline">
                <tr><th>Release Date</th><td>2023-01-01</td></tr>
                <tr><th>Registration Date</th><td>2023-01-01</td></tr>
                <tr><th>Update Date</th><td>2023-02-01</td></tr>
                <tr><th>Series</th><td>Test Series</td></tr>
                <tr><th>Page Count</th><td>24 pages</td></tr>
                <tr><th>File size</th><td>100MB</td></tr>
                <tr>
                    <th>Voice Actor</th>
                    <td><a href="#">Actor 1</a>, <a href="#">Actor 2</a></td>
                </tr>
                <tr>
                    <th>Author</th>
                    <td><a href="#">Author 1</a></td>
                </tr>
                <tr>
                    <th>Genre</th>
                    <td><a href="#">RPG</a> <a href="#">Action</a></td>
                </tr>
            </table>
            
            <div class="work_parts_container">Test Description</div>
        </body>
        </html>
        "##;

        let data = parse_html_response(html).expect("Failed to parse HTML");

        assert_eq!(data.title, Some("Test Title".to_string()));
        assert_eq!(data.circle, Some("Test Circle".to_string()));
        assert_eq!(data.brand, Some("Test Brand".to_string()));
        assert_eq!(data.publisher, Some("Test Publisher".to_string()));
        assert_eq!(data.release_date, Some("2023-01-01".to_string()));
        assert_eq!(data.update_date, Some("2023-02-01".to_string()));
        assert_eq!(data.series, Some("Test Series".to_string()));
        assert_eq!(data.page_count, Some(24));
        assert_eq!(data.file_size, Some("100MB".to_string()));
        assert_eq!(data.voice_actors, vec!["Actor 1", "Actor 2"]);
        assert_eq!(data.authors, vec!["Author 1"]);
        assert_eq!(data.genres, vec!["RPG", "Action"]);
        assert_eq!(data.description, Some("Test Description".to_string()));
        assert!(!data.geo_blocked, "Normal page should not be geo-blocked");
    }

    #[test]
    fn test_geo_blocked_empty_content() {
        // Minimal HTML with no useful content - should be flagged as geo-blocked
        let html = r#"
        <html>
        <body>
            <h1>Page Title</h1>
            <p>Some random content with no product info</p>
        </body>
        </html>
        "#;

        let data = parse_html_response(html).expect("Failed to parse HTML");
        assert!(
            data.geo_blocked,
            "Page with no essential content should be geo-blocked"
        );
    }

    #[test]
    fn test_geo_blocked_japanese_message() {
        // Japanese geo-block message
        let html = r#"
        <html>
        <body>
            <h1 id="work_name">Some Title</h1>
            <p>お住いの国・地域からは本作品は購入できません</p>
        </body>
        </html>
        "#;

        let data = parse_html_response(html).expect("Failed to parse HTML");
        assert!(
            data.geo_blocked,
            "Japanese geo-block message should be detected"
        );
    }

    #[test]
    fn test_geo_blocked_english_message() {
        // English geo-block message
        let html = r#"
        <html>
        <body>
            <h1 id="work_name">Some Title</h1>
            <p>This product cannot be purchased from your region.</p>
        </body>
        </html>
        "#;

        let data = parse_html_response(html).expect("Failed to parse HTML");
        assert!(
            data.geo_blocked,
            "English geo-block message should be detected"
        );
    }

    #[test]
    fn test_geo_blocked_no_essential_fields() {
        // Has work_name but no cover, no genres, no circle, no description
        let html = r#"
        <html>
        <body>
            <h1 id="work_name">Some Title</h1>
            <table id="work_outline">
                <tr><th>Release Date</th><td>2023-01-01</td></tr>
            </table>
        </body>
        </html>
        "#;

        let data = parse_html_response(html).expect("Failed to parse HTML");
        assert!(
            data.geo_blocked,
            "Page with no essential content should be geo-blocked"
        );
    }

    #[test]
    fn test_not_geo_blocked_with_circle() {
        // Has circle - should NOT be geo-blocked
        let html = r##"
        <html>
        <body>
            <h1 id="work_name">Test Title</h1>
            <div id="work_right">
                <span class="maker_name"><a href="#">Test Circle</a></span>
            </div>
        </body>
        </html>
        "##;

        let data = parse_html_response(html).expect("Failed to parse HTML");
        assert!(
            !data.geo_blocked,
            "Page with circle should not be geo-blocked"
        );
        assert_eq!(data.circle, Some("Test Circle".to_string()));
    }

    #[test]
    fn test_not_geo_blocked_with_description() {
        // Has description - should NOT be geo-blocked
        let html = r#"
        <html>
        <body>
            <h1 id="work_name">Test Title</h1>
            <div class="work_parts_container">This is a product description.</div>
        </body>
        </html>
        "#;

        let data = parse_html_response(html).expect("Failed to parse HTML");
        assert!(
            !data.geo_blocked,
            "Page with description should not be geo-blocked"
        );
    }

    #[test]
    fn test_not_geo_blocked_with_genres() {
        // Has genres - should NOT be geo-blocked
        let html = r##"
        <html>
        <body>
            <h1 id="work_name">Test Title</h1>
            <table id="work_outline">
                <tr>
                    <th>Genre</th>
                    <td><a href="#">RPG</a></td>
                </tr>
            </table>
        </body>
        </html>
        "##;

        let data = parse_html_response(html).expect("Failed to parse HTML");
        assert!(
            !data.geo_blocked,
            "Page with genres should not be geo-blocked"
        );
        assert!(!data.genres.is_empty());
    }
}
