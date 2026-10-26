import XCTest
@testable import MalacKit

final class MarkdownTests: XCTestCase {
    func testTheRecordedAnswer() throws {
        let messages = try Fixtures.received()
        let answer = messages.lazy.compactMap { message -> String? in
            guard case .event(_, _, let envelope) = message, case let .turnFinished(.completed, text) = envelope.event,
                  text.hasPrefix("## Summary") else { return nil }
            return text
        }.first
        let blocks = Markdown.parse(try XCTUnwrap(answer))
        guard case .heading(2, "Summary") = blocks.first else { return XCTFail("heading") }
        guard case .list(let list) = blocks[1] else { return XCTFail("list") }
        XCTAssertFalse(list.ordered)
        XCTAssertGreaterThanOrEqual(list.items.count, 3)
        XCTAssertTrue(blocks.contains { if case .code("rust", let code) = $0 { return code.contains("println!") }; return false })
    }

    func testBlocks() {
        let source = """
        # Title

        First line
        second line with `code`

        > quoted
        > text

        ---

        ```swift
        let x = 1
        ```

        | Name | Size |
        |:-----|-----:|
        | a \\| b | 2 |
        """
        let blocks = Markdown.parse(source)
        XCTAssertEqual(blocks, [
            .heading(level: 1, text: "Title"),
            .paragraph("First line\nsecond line with `code`"),
            .quote([.paragraph("quoted\ntext")]),
            .rule,
            .code(language: "swift", code: "let x = 1"),
            .table(MarkdownTable(header: ["Name", "Size"], alignments: [.leading, .trailing], rows: [["a | b", "2"]])),
        ])
    }

    func testListsNestAndNumber() {
        let blocks = Markdown.parse("""
        3. three
        4. four
           - nested
           - [x] done

        5. five
        """)
        guard case .list(let list) = blocks.first, blocks.count == 1 else { return XCTFail("one list: \(blocks)") }
        XCTAssertTrue(list.ordered)
        XCTAssertEqual(list.start, 3)
        XCTAssertTrue(list.loose)
        XCTAssertEqual(list.items.count, 3)
        guard case .list(let nested) = list.items[1].blocks.last else { return XCTFail("nested list") }
        XCTAssertEqual(nested.items.map(\.task), [nil, true])
        XCTAssertEqual(nested.items[1].blocks, [.paragraph("done")])
    }

    func testAnUnclosedFenceIsCodeSoFar() {
        XCTAssertEqual(Markdown.parse("Here:\n```py\nprint(1)"), [.paragraph("Here:"), .code(language: "py", code: "print(1)")])
        XCTAssertEqual(Markdown.parse("````\n```\nnested\n````"), [.code(language: "", code: "```\nnested")])
    }
}
