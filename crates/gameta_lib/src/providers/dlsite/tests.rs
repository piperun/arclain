#[cfg(test)]
mod tests {
    use super::super::parse_html_response;

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
        assert!(!data.geo_blocked);
    }

    #[test]
    fn test_geo_blocked_detection() {
        let html = r#"
        <html>
        <body>
            <h1 id="work_name">Some Title</h1>
            <p>お住いの国・地域からは本作品は購入できません</p>
        </body>
        </html>
        "#;

        let data = parse_html_response(html).expect("Failed to parse HTML");
        assert!(data.geo_blocked);
    }

    #[test]
    fn test_not_geo_blocked_with_circle() {
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
        assert!(!data.geo_blocked);
        assert_eq!(data.circle, Some("Test Circle".to_string()));
    }
}
