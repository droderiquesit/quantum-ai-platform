const path = require("path");
const CopyPlugin = require("copy-webpack-plugin");
const MiniCssExtractPlugin = require("mini-css-extract-plugin");
const fs = require("fs");
const HtmlWebpackPlugin = require("html-webpack-plugin");
const HtmlWebpackSimpleIncludePlugin = require("html-webpack-simple-include-plugin");
const autoprefixer = require("autoprefixer");

// Define the root directory containing the HTML files
const rootDirectory = path.resolve(__dirname, "src");

// Function to generate HtmlWebpackPlugin instances for each HTML file
function generateHtmlPlugins(rootDir) {
  const plugins = [];

  // Recursive function to find HTML files in all subdirectories
  function scanDirectory(dir) {
    const files = fs.readdirSync(dir);

    files.forEach((file) => {
      const fullPath = path.join(dir, file);
      const stat = fs.statSync(fullPath);

      if (stat.isDirectory()) {
        // Recursively scan subdirectories
        scanDirectory(fullPath);
      } else if (path.extname(file) === ".html") {
        // Get relative path from src directory for output
        const relativePath = path.relative(rootDir, fullPath);

        plugins.push(
          new HtmlWebpackPlugin({
            filename: relativePath,
            template: fullPath,
            inject: "body",
          }),
        );
      }
    });
  }

  scanDirectory(rootDir);
  return plugins;
}

const htmlFiles = generateHtmlPlugins(rootDirectory);
//partial files
const partialFiles = ["sidebar", "top-header"].map((partial) => {
  return {
    tag: `<include-${partial} />`,
    content: fs.readFileSync(
      path.resolve(__dirname, `src/partials/${partial}.html`),
    ),
  };
});

module.exports = {
  entry: {
    main: "./src/js/index.js",
  },
  mode: "development",
  devServer: {
    watchFiles: ["./src/**/*"],
    hot: true,
    port: 5001,
  },
  module: {
    rules: [
      {
        test: /\.css$/i,
        use: [MiniCssExtractPlugin.loader, "css-loader", "postcss-loader"],
      },
      {
        test: /\.(png|svg|jpg|jpeg|gif)$/i,
        type: "asset/resource",
      },
      {
        test: /\.(scss)$/,
        use: [
          {
            // Adds CSS to the DOM by injecting a `<style>` tag
            loader: MiniCssExtractPlugin.loader,
          },
          {
            // Interprets `@import` and `url()` like `import/require()` and will resolve them
            loader: "css-loader",
            options: {
              url: false,
            },
          },
          {
            // Loader for webpack to process CSS with PostCSS
            loader: "postcss-loader",
            options: {
              postcssOptions: {
                plugins: [autoprefixer],
              },
            },
          },
          {
            // Loads a SASS/SCSS file and compiles it to CSS
            loader: "sass-loader",
          },
        ],
      },
      {
        test: /\.(woff|woff2|eot|ttf|otf)$/i, // Match font files
        type: "asset/resource", // Webpack 5 way to handle assets
        generator: {
          filename: "assets/fonts/[name][ext]", // Output to 'assets/fonts'
        },
      },
    ],
  },
  resolve: {
    extensions: [".tsx", ".ts", ".js"],
  },
  plugins: [
    new MiniCssExtractPlugin({
      filename: "css/index.css",
    }),
    new CopyPlugin({
      patterns: [
        { from: "src/assets", to: "assets" },
        // { from: "src/manifest.json", to: "manifest.json" },
        // { from: "src/service-worker.js", to: "service-worker.js" },
      ],
    }),
    ...htmlFiles,
    new HtmlWebpackSimpleIncludePlugin([...partialFiles]),
  ],
  output: {
    filename: "js/index.js",
    path: path.resolve(__dirname, "dist"),
    clean: true,
  },
};
